#include "support/fake_anyconnect_server.hpp"

#include "vpn_engine/protocol/session.hpp"

#include <cstdint>
#include <iostream>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

std::vector<std::uint8_t> bytes(std::initializer_list<std::uint8_t> values) {
  return std::vector<std::uint8_t>(values.begin(), values.end());
}

bool contains_event(
    const ecnuvpn::tests::support::RecordingEventSink &events,
    const std::string &type) {
  for (const auto &event : events.events()) {
    if (event.type == type)
      return true;
  }
  return false;
}

bool events_contain_password(
    const ecnuvpn::tests::support::RecordingEventSink &events,
    const std::string &password) {
  for (const auto &event : events.events()) {
    if (event.message.find(password) != std::string::npos)
      return true;
    for (const auto &field : event.fields) {
      if (field.first.find(password) != std::string::npos ||
          field.second.find(password) != std::string::npos) {
        return true;
      }
    }
  }
  return false;
}

class FakeProtocolTransport final
    : public ecnuvpn::vpn_engine::protocol::ProtocolTransport {
public:
  explicit FakeProtocolTransport(
      ecnuvpn::tests::support::FakeAnyConnectServer &server)
      : server_(server) {}

  ecnuvpn::vpn_engine::protocol::AuthResult authenticate(
      const ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions
          &options) override {
    ecnuvpn::tests::support::FakeAnyConnectCredentials credentials;
    credentials.username = options.username;
    credentials.password = options.password;
    return server_.password_authenticate(credentials);
  }

  ecnuvpn::vpn_engine::ValidationResult
  connect_cstp(const std::string &cookie,
               ecnuvpn::vpn_engine::TunnelMetadata *metadata) override {
    return server_.connect_cstp(cookie, metadata);
  }

  ecnuvpn::vpn_engine::ValidationResult
  exchange_packet(const std::vector<std::uint8_t> &packet,
                  std::vector<std::uint8_t> *response_packet) override {
    return server_.exchange_packet(packet, response_packet);
  }

  void disconnect() override { server_.reset_transport(); }

  void reset_for_reconnect() override { server_.reset_transport(); }

private:
  ecnuvpn::tests::support::FakeAnyConnectServer &server_;
};

class ManualCancellationToken final
    : public ecnuvpn::vpn_engine::protocol::CancellationToken {
public:
  bool is_cancelled() const override { return cancelled_; }
  void cancel() { cancelled_ = true; }

private:
  bool cancelled_ = false;
};

class NoDataCancellingPacketDevice final : public ecnuvpn::vpn_engine::PacketDevice {
public:
  NoDataCancellingPacketDevice(ManualCancellationToken &cancel,
                               int cancel_after_reads)
      : cancel_(cancel), cancel_after_reads_(cancel_after_reads) {}

  ecnuvpn::vpn_engine::ValidationResult
  open(const ecnuvpn::vpn_engine::TunnelMetadata &metadata) override {
    last_open_metadata_ = metadata;
    open_ = true;
    ++open_count_;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override {
    if (!packet)
      return {false, "packet_null_out", "packet output must not be null"};
    if (!open_)
      return {false, "packet_device_closed", "packet device is closed"};

    packet->clear();
    ++read_count_;
    if (read_count_ >= cancel_after_reads_)
      cancel_.cancel();
    return {false, "no_data", "no packet available"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> & /*packet*/) override {
    return {false, "unexpected_write", "no packets should be written"};
  }

  void close() override {
    if (open_) {
      open_ = false;
      ++close_count_;
    }
  }

  int open_count() const { return open_count_; }
  int close_count() const { return close_count_; }
  int read_count() const { return read_count_; }
  const ecnuvpn::vpn_engine::TunnelMetadata &last_open_metadata() const {
    return last_open_metadata_;
  }

private:
  ManualCancellationToken &cancel_;
  int cancel_after_reads_ = 0;
  ecnuvpn::vpn_engine::TunnelMetadata last_open_metadata_;
  bool open_ = false;
  int open_count_ = 0;
  int close_count_ = 0;
  int read_count_ = 0;
};

ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions session_options() {
  ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions options;
  options.server.scheme = "https";
  options.server.host = "vpn.example.invalid";
  options.server.port = 443;
  options.server.base_path = "/";
  options.username = "alice";
  options.password = "correct-password";
  options.useragent = "ECNU-VPN test";
  options.disable_dtls = true;
  return options;
}

bool test_authenticate_success_and_connect_cstp() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  FakeAnyConnectServer server;
  FakeProtocolTransport transport(server);
  ProtocolSession session(session_options(), &transport);

  auto auth = session.authenticate();
  ok = expect(auth.ok, "auth success should return ok") && ok;
  ok = expect(server.auth_attempts() == 1,
              "auth success should call transport authenticate once") &&
       ok;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  auto cstp = session.connect_cstp(&metadata);
  ok = expect(cstp.ok, "CSTP connect should return ok after auth") && ok;
  ok = expect(metadata.interface_name == "fake-cstp0",
              "CSTP connect should return tunnel metadata") &&
       ok;
  ok = expect(session.state().phase ==
                  ecnuvpn::vpn_engine::SessionPhase::configuring_network,
              "CSTP connect should configure session state") &&
       ok;

  return ok;
}

bool test_auth_failure_never_reconnects() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  FakeAnyConnectServer server;
  FakeProtocolTransport transport(server);

  auto options = session_options();
  options.password = "wrong-password";
  options.auto_reconnect = true;
  options.max_reconnects = 3;

  ProtocolSession session(options, &transport);
  auto auth = session.authenticate();

  ok = expect(!auth.ok, "bad credentials should fail auth") && ok;
  ok = expect(auth.code == "auth_failed",
              "bad credentials should return auth_failed") &&
       ok;
  ok = expect(server.auth_attempts() == 1,
              "auth failure should not retry or reconnect") &&
       ok;
  ok = expect(server.cstp_connects() == 0,
              "auth failure should not advance to CSTP") &&
       ok;
  ok = expect(session.reconnect_attempts() == 0,
              "auth failure should not increment reconnect attempts") &&
       ok;

  return ok;
}

bool test_packet_echo() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  FakeAnyConnectServer server;
  FakeProtocolTransport transport(server);
  RecordingEventSink events;
  ScriptedPacketDevice device({bytes({0x45, 0x00, 0x00, 0x2a})});
  ManualCancellationToken cancel;

  ProtocolSession session(session_options(), &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok, "auth should succeed before packet loop") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP connect should succeed before packet loop") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(loop.ok, "packet echo loop should finish cleanly") && ok;
  ok = expect(session.state().network_ready(),
              "packet loop should mark network ready") &&
       ok;
  ok = expect(device.open_count() == 1,
              "packet loop should open the packet device") &&
       ok;
  ok = expect(device.written_packets().size() == 1 &&
                  device.written_packets()[0] ==
                      bytes({0x45, 0x00, 0x00, 0x2a}),
              "packet loop should echo packet bytes to device") &&
       ok;
  ok = expect(contains_event(events, "packet.echo"),
              "packet loop should emit packet.echo") &&
       ok;
  ok = expect(!events_contain_password(events, "correct-password"),
              "event stream must not include password") &&
       ok;

  return ok;
}

bool test_reconnect_after_transport_close() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;

  FakeAnyConnectServerOptions server_options;
  server_options.close_on_data_frame_number = 1;
  server_options.close_only_once = true;

  FakeAnyConnectServer server(server_options);
  FakeProtocolTransport transport(server);
  RecordingEventSink events;
  ScriptedPacketDevice device(
      {bytes({0xaa, 0xbb, 0xcc}), bytes({0x45, 0x11, 0x22, 0x33})});
  ManualCancellationToken cancel;

  auto options = session_options();
  options.auto_reconnect = true;
  options.max_reconnects = 1;

  ProtocolSession session(options, &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok, "initial auth should succeed") && ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "initial CSTP connect should succeed") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(loop.ok, "reconnect loop should finish cleanly") && ok;
  ok = expect(session.reconnect_attempts() == 1,
              "one reconnect should be attempted") &&
       ok;
  ok = expect(server.auth_attempts() == 2,
              "reconnect should re-authenticate") &&
       ok;
  ok = expect(server.cstp_connects() == 2,
              "reconnect should repeat CSTP connect") &&
       ok;
  ok = expect(device.open_count() == 2,
              "reconnect should reopen packet device") &&
       ok;
  ok = expect(device.close_count() == 1,
              "reconnect should close the old packet device") &&
       ok;
  ok = expect(device.written_packets().size() == 1 &&
                  device.written_packets()[0] ==
                      bytes({0x45, 0x11, 0x22, 0x33}),
              "post-reconnect packet should be echoed") &&
       ok;
  ok = expect(contains_event(events, "reconnect_started"),
              "reconnect should emit reconnect_started") &&
       ok;
  ok = expect(contains_event(events, "reconnect_succeeded"),
              "reconnect should emit reconnect_succeeded") &&
       ok;

  return ok;
}

bool test_cancellation_exits_without_opening_device() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  FakeAnyConnectServer server;
  FakeProtocolTransport transport(server);
  RecordingEventSink events;
  ScriptedPacketDevice device({bytes({0x01})});
  ManualCancellationToken cancel;
  cancel.cancel();

  ProtocolSession session(session_options(), &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok, "auth should succeed before cancellation") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before cancellation") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "session_cancelled",
              "cancelled loop should return session_cancelled") &&
       ok;
  ok = expect(session.state().phase == ecnuvpn::vpn_engine::SessionPhase::stopped,
              "cancelled loop should mark session stopped") &&
       ok;
  ok = expect(device.open_count() == 0,
              "cancelled loop should not open packet device") &&
       ok;
  ok = expect(device.written_packets().empty(),
              "cancelled loop should not write packets") &&
       ok;

  return ok;
}

bool test_active_loop_cancellation_during_no_data_poll_exits() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  FakeAnyConnectServer server;
  FakeProtocolTransport transport(server);
  RecordingEventSink events;
  ManualCancellationToken cancel;
  NoDataCancellingPacketDevice device(cancel, 3);

  ProtocolSession session(session_options(), &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok,
              "auth should succeed before active cancellation") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before active cancellation") &&
       ok;

  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "session_cancelled",
              "active cancelled loop should return session_cancelled") &&
       ok;
  ok = expect(device.open_count() == 1,
              "active loop should open the packet device") &&
       ok;
  ok = expect(device.read_count() >= 3,
              "active loop should retry no_data reads until cancellation") &&
       ok;
  ok = expect(device.close_count() == 1,
              "active cancellation should close the packet device") &&
       ok;
  ok = expect(device.last_open_metadata().interface_name == "fake-cstp0",
              "active loop should pass tunnel metadata to packet device") &&
       ok;
  ok = expect(session.state().phase == ecnuvpn::vpn_engine::SessionPhase::stopped,
              "active cancellation should mark session stopped") &&
       ok;

  return ok;
}

bool test_disconnect_stops_before_packet_loop() {
  using namespace ecnuvpn::tests::support;
  using ecnuvpn::vpn_engine::protocol::ProtocolSession;

  bool ok = true;
  FakeAnyConnectServer server;
  FakeProtocolTransport transport(server);
  RecordingEventSink events;
  ScriptedPacketDevice device({bytes({0x01})});
  ManualCancellationToken cancel;

  ProtocolSession session(session_options(), &transport);

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(session.authenticate().ok, "auth should succeed before disconnect") &&
       ok;
  ok = expect(session.connect_cstp(&metadata).ok,
              "CSTP should succeed before disconnect") &&
       ok;

  session.disconnect();
  auto loop = session.run_packet_loop(&device, &events, &cancel);

  ok = expect(!loop.ok && loop.code == "session_cancelled",
              "disconnected loop should return session_cancelled") &&
       ok;
  ok = expect(session.state().phase == ecnuvpn::vpn_engine::SessionPhase::stopped,
              "disconnect should mark session stopped") &&
       ok;
  ok = expect(device.open_count() == 0,
              "disconnect before loop should not open packet device") &&
       ok;

  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = test_authenticate_success_and_connect_cstp() && ok;
  ok = test_auth_failure_never_reconnects() && ok;
  ok = test_packet_echo() && ok;
  ok = test_reconnect_after_transport_close() && ok;
  ok = test_cancellation_exits_without_opening_device() && ok;
  ok = test_active_loop_cancellation_during_no_data_poll_exits() && ok;
  ok = test_disconnect_stops_before_packet_loop() && ok;

  return ok ? 0 : 1;
}
