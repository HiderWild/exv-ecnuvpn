#include "support/fake_anyconnect_server.hpp"

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

bool test_password_auth_success() {
  using namespace ecnuvpn::tests::support;

  bool ok = true;
  FakeAnyConnectServer server;

  const auto result = server.password_authenticate(FakeAnyConnectCredentials{});
  ok = expect(result.ok, "password auth should succeed with expected credentials") && ok;
  ok = expect(result.cookie == "webvpn=FAKE_COOKIE",
              "password auth should return the webvpn cookie") &&
       ok;
  ok = expect(server.auth_attempts() == 1, "auth attempt count should increment") && ok;

  return ok;
}

bool test_password_auth_failure() {
  using namespace ecnuvpn::tests::support;

  bool ok = true;
  FakeAnyConnectServer server;

  FakeAnyConnectCredentials credentials;
  credentials.password = "test-mock-wrong-password";

  const auto result = server.password_authenticate(credentials);
  ok = expect(!result.ok, "password auth should reject bad credentials") && ok;
  ok = expect(result.error_code == "auth_failed",
              "bad password should map to auth_failed") &&
       ok;
  ok = expect(result.error_message.find("invalid") != std::string::npos,
              "bad password should preserve server error detail") &&
       ok;
  ok = expect(server.cstp_connects() == 0,
              "auth failure must not advance to CSTP connect") &&
       ok;

  return ok;
}

bool test_cstp_connect_success() {
  using namespace ecnuvpn::tests::support;

  bool ok = true;
  FakeAnyConnectServer server;

  const auto auth = server.password_authenticate(FakeAnyConnectCredentials{});
  ok = expect(auth.ok, "auth should succeed before CSTP connect") && ok;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  const auto connected = server.connect_cstp(auth.cookie, &metadata);
  ok = expect(connected.ok, "CSTP connect should succeed with auth cookie") && ok;
  ok = expect(metadata.interface_name == "fake-cstp0",
              "fake CSTP metadata should include a test-only interface name") &&
       ok;
  ok = expect(metadata.internal_ip4_address == "10.255.0.10",
              "CSTP metadata should parse internal address") &&
       ok;
  ok = expect(metadata.mtu == 1400, "CSTP metadata should parse MTU") && ok;
  ok = expect(metadata.routes.size() == 2, "CSTP split routes should parse") && ok;
  ok = expect(server.cstp_connects() == 1,
              "CSTP connect count should increment") &&
       ok;

  return ok;
}

bool test_packet_echo() {
  using namespace ecnuvpn::tests::support;

  bool ok = true;
  FakeAnyConnectServer server;
  ScriptedPacketDevice device({bytes({0x45, 0x00, 0x00, 0x2a})});
  RecordingEventSink events;

  FakeAnyConnectRunOptions options;
  options.credentials = FakeAnyConnectCredentials{};

  const auto result = run_fake_anyconnect_session(server, device, events, options);

  ok = expect(result.result.ok, "packet echo scenario should finish cleanly") && ok;
  ok = expect(result.state.network_ready(),
              "packet echo scenario should reach network-ready packet loop") &&
       ok;
  ok = expect(device.written_packets().size() == 1,
              "device should receive one echoed packet") &&
       ok;
  ok = expect(device.written_packets().size() == 1 &&
                  device.written_packets()[0] == bytes({0x45, 0x00, 0x00, 0x2a}),
              "echoed packet bytes should match") &&
       ok;
  ok = expect(contains_event(events, "packet.echo"),
              "packet echo should emit a packet.echo event") &&
       ok;

  return ok;
}

bool test_server_close_during_packet_loop() {
  using namespace ecnuvpn::tests::support;

  bool ok = true;

  FakeAnyConnectServerOptions server_options;
  server_options.close_on_data_frame_number = 1;

  FakeAnyConnectServer server(server_options);
  ScriptedPacketDevice device({bytes({0x01, 0x02, 0x03})});
  RecordingEventSink events;

  FakeAnyConnectRunOptions options;
  options.credentials = FakeAnyConnectCredentials{};
  options.auto_reconnect = false;

  const auto result = run_fake_anyconnect_session(server, device, events, options);

  ok = expect(!result.result.ok, "server close should fail without reconnect") && ok;
  ok = expect(result.result.code == "transport_closed",
              "server close should surface transport_closed") &&
       ok;
  ok = expect(result.state.phase == ecnuvpn::vpn_engine::SessionPhase::failed,
              "server close should mark the session failed") &&
       ok;
  ok = expect(device.close_count() == 1,
              "device should be closed after transport failure") &&
       ok;
  ok = expect(device.written_packets().empty(),
              "packet should not be echoed when server closes before response") &&
       ok;
  ok = expect(contains_event(events, "transport.closed"),
              "server close should emit transport.closed") &&
       ok;

  return ok;
}

bool test_reconnect_after_close() {
  using namespace ecnuvpn::tests::support;

  bool ok = true;

  FakeAnyConnectServerOptions server_options;
  server_options.close_on_data_frame_number = 1;
  server_options.close_only_once = true;

  FakeAnyConnectServer server(server_options);
  ScriptedPacketDevice device(
      {bytes({0xaa, 0xbb, 0xcc}), bytes({0x45, 0x11, 0x22, 0x33})});
  RecordingEventSink events;

  FakeAnyConnectRunOptions options;
  options.credentials = FakeAnyConnectCredentials{};
  options.auto_reconnect = true;
  options.max_reconnects = 1;

  const auto result = run_fake_anyconnect_session(server, device, events, options);

  ok = expect(result.result.ok, "reconnect scenario should finish cleanly") && ok;
  ok = expect(result.reconnects == 1, "one reconnect should be attempted") && ok;
  ok = expect(server.auth_attempts() == 2,
              "reconnect should repeat password auth") &&
       ok;
  ok = expect(server.cstp_connects() == 2,
              "reconnect should repeat CSTP connect") &&
       ok;
  ok = expect(device.open_count() == 2,
              "packet device should be reopened after reconnect") &&
       ok;
  ok = expect(result.state.network_ready(),
              "session should return to packet loop after reconnect") &&
       ok;
  ok = expect(device.written_packets().size() == 1,
              "one packet should be echoed after reconnect") &&
       ok;
  ok = expect(device.written_packets().size() == 1 &&
                  device.written_packets()[0] == bytes({0x45, 0x11, 0x22, 0x33}),
              "post-reconnect packet should echo") &&
       ok;
  ok = expect(contains_event(events, "reconnect.started"),
              "reconnect should emit reconnect.started") &&
       ok;
  ok = expect(contains_event(events, "reconnect.succeeded"),
              "reconnect should emit reconnect.succeeded") &&
       ok;

  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = test_password_auth_success() && ok;
  ok = test_password_auth_failure() && ok;
  ok = test_cstp_connect_success() && ok;
  ok = test_packet_echo() && ok;
  ok = test_server_close_during_packet_loop() && ok;
  ok = test_reconnect_after_close() && ok;

  return ok ? 0 : 1;
}
