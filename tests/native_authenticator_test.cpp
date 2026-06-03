#include "vpn_engine/protocol/native_authenticator.hpp"

#include <cstdint>
#include <iostream>
#include <map>
#include <string>
#include <vector>

namespace {

static const char *MOCK_PASSWORD = "test-mock-password-placeholder";

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

bool diagnostics_contain(const std::map<std::string, std::string> &diagnostics,
                         const std::string &needle) {
  for (const auto &entry : diagnostics) {
    if (entry.first.find(needle) != std::string::npos ||
        entry.second.find(needle) != std::string::npos) {
      return true;
    }
  }
  return false;
}

ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions options() {
  ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions out;
  out.server.scheme = "https";
  out.server.host = "vpn.example.invalid";
  out.server.port = 443;
  out.server.base_path = "/";
  out.username = "student@example.invalid";
  out.password = MOCK_PASSWORD;
  out.useragent = "ECNU-VPN native-auth test";
  return out;
}

class FakeProtocolTransport final
    : public ecnuvpn::vpn_engine::protocol::ProtocolTransport {
public:
  ecnuvpn::vpn_engine::protocol::AuthResult authenticate(
      const ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions
          &options) override {
    ++authenticate_calls;
    last_options = options;
    return auth_result;
  }

  ecnuvpn::vpn_engine::ValidationResult
  connect_cstp(const std::string & /*cookie*/,
               ecnuvpn::vpn_engine::TunnelMetadata * /*metadata*/) override {
    ++connect_cstp_calls;
    return {false, "unexpected_cstp", "CSTP must not be called by auth"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  send_packet(const std::vector<std::uint8_t> & /*packet*/) override {
    return {false, "unexpected_packet", "packet path must not be called"};
  }

  ecnuvpn::vpn_engine::ValidationResult
  send_control(ecnuvpn::vpn_engine::protocol::InboundFrameKind /*kind*/)
      override {
    return {false, "unexpected_control", "control path must not be called"};
  }

  ecnuvpn::vpn_engine::ValidationResult receive_frame(
      ecnuvpn::vpn_engine::protocol::InboundFrame * /*out*/) override {
    return {false, "unexpected_receive", "receive path must not be called"};
  }

  void disconnect() override { ++disconnect_calls; }
  void reset_for_reconnect() override { ++reset_calls; }

  ecnuvpn::vpn_engine::protocol::AuthResult auth_result;
  ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions last_options;
  int authenticate_calls = 0;
  int connect_cstp_calls = 0;
  int disconnect_calls = 0;
  int reset_calls = 0;
};

bool fake_transport_success_populates_session() {
  using ecnuvpn::vpn_engine::protocol::NativeAuthenticator;
  using ecnuvpn::vpn_engine::protocol::NativeAuthRequest;
  using ecnuvpn::vpn_engine::protocol::NativeAuthSession;

  bool ok = true;
  FakeProtocolTransport transport;
  transport.auth_result.ok = true;
  transport.auth_result.cookie = "webvpn=SESSION";

  NativeAuthenticator authenticator(&transport);
  NativeAuthSession session;
  const auto result =
      authenticator.authenticate(NativeAuthRequest{options()}, &session);

  ok = expect(result.ok, "successful auth should return ok") && ok;
  ok = expect(transport.authenticate_calls == 1,
              "success should call transport authenticate once") &&
       ok;
  ok = expect(transport.connect_cstp_calls == 0,
              "success should not connect CSTP") &&
       ok;
  ok = expect(session.cookie_header == "webvpn=SESSION",
              "session should carry auth cookie header") &&
       ok;
  ok = expect(session.server.host == "vpn.example.invalid",
              "session should carry server host") &&
       ok;
  ok = expect(session.server.base_path == "/",
              "session should carry server base path") &&
       ok;
  ok = expect(session.username == "student@example.invalid",
              "session should carry username") &&
       ok;
  ok = expect(session.useragent == "ECNU-VPN native-auth test",
              "session should carry user agent") &&
       ok;
  ok = expect(session.auth_method == "password",
              "session should default to password auth method") &&
       ok;
  ok = expect(session.created_at !=
                  std::chrono::system_clock::time_point{},
              "session should record creation time") &&
       ok;
  return ok;
}

bool auth_failed_propagates_exact_error_without_cstp() {
  using ecnuvpn::vpn_engine::protocol::NativeAuthenticator;
  using ecnuvpn::vpn_engine::protocol::NativeAuthRequest;
  using ecnuvpn::vpn_engine::protocol::NativeAuthSession;

  bool ok = true;
  FakeProtocolTransport transport;
  transport.auth_result.ok = false;
  transport.auth_result.error_code = "auth_failed";
  transport.auth_result.error_message = "gateway rejected credentials";

  NativeAuthenticator authenticator(&transport);
  NativeAuthSession session;
  const auto result =
      authenticator.authenticate(NativeAuthRequest{options()}, &session);

  ok = expect(!result.ok, "auth_failed should return failure") && ok;
  ok = expect(result.code == "auth_failed",
              "auth_failed should preserve exact code") &&
       ok;
  ok = expect(result.message == "gateway rejected credentials",
              "auth_failed should preserve exact message") &&
       ok;
  ok = expect(transport.authenticate_calls == 1,
              "failure should call transport authenticate once") &&
       ok;
  ok = expect(transport.connect_cstp_calls == 0,
              "failure should not connect CSTP") &&
       ok;
  return ok;
}

bool auth_protocol_mismatch_propagates_exact_code() {
  using ecnuvpn::vpn_engine::protocol::NativeAuthenticator;
  using ecnuvpn::vpn_engine::protocol::NativeAuthRequest;
  using ecnuvpn::vpn_engine::protocol::NativeAuthSession;

  bool ok = true;
  FakeProtocolTransport transport;
  transport.auth_result.ok = false;
  transport.auth_result.error_code = "auth_protocol_mismatch";
  transport.auth_result.error_message = "unexpected config-auth response";

  NativeAuthenticator authenticator(&transport);
  NativeAuthSession session;
  const auto result =
      authenticator.authenticate(NativeAuthRequest{options()}, &session);

  ok = expect(!result.ok, "protocol mismatch should return failure") && ok;
  ok = expect(result.code == "auth_protocol_mismatch",
              "protocol mismatch should preserve exact code") &&
       ok;
  ok = expect(transport.connect_cstp_calls == 0,
              "protocol mismatch should not connect CSTP") &&
       ok;
  return ok;
}

bool plaintext_password_is_not_added_to_diagnostics_or_errors() {
  using ecnuvpn::vpn_engine::protocol::NativeAuthenticator;
  using ecnuvpn::vpn_engine::protocol::NativeAuthRequest;
  using ecnuvpn::vpn_engine::protocol::NativeAuthSession;

  bool ok = true;
  auto request_options = options();
  request_options.password = MOCK_PASSWORD;

  FakeProtocolTransport success_transport;
  success_transport.auth_result.ok = true;
  success_transport.auth_result.cookie = "webvpn=SESSION";

  NativeAuthenticator success_authenticator(&success_transport);
  NativeAuthSession success_session;
  const auto success = success_authenticator.authenticate(
      NativeAuthRequest{request_options}, &success_session);

  ok = expect(success.ok, "success path should authenticate") && ok;
  ok = expect(!diagnostics_contain(success_session.diagnostics,
                                   request_options.password),
              "success diagnostics must not contain plaintext password") &&
       ok;

  FakeProtocolTransport failure_transport;
  failure_transport.auth_result.ok = false;
  failure_transport.auth_result.error_code = "auth_failed";
  failure_transport.auth_result.error_message = "authentication failed";

  NativeAuthenticator failure_authenticator(&failure_transport);
  NativeAuthSession failure_session;
  const auto failure = failure_authenticator.authenticate(
      NativeAuthRequest{request_options}, &failure_session);

  ok = expect(!failure.ok, "failure path should fail authentication") && ok;
  ok = expect(failure.message.find(request_options.password) ==
                  std::string::npos,
              "failure error message must not contain plaintext password") &&
       ok;
  ok = expect(!diagnostics_contain(failure_session.diagnostics,
                                   request_options.password),
              "failure diagnostics must not contain plaintext password") &&
       ok;
  return ok;
}

bool missing_transport_or_output_is_rejected() {
  using ecnuvpn::vpn_engine::protocol::NativeAuthenticator;
  using ecnuvpn::vpn_engine::protocol::NativeAuthRequest;
  using ecnuvpn::vpn_engine::protocol::NativeAuthSession;

  bool ok = true;
  NativeAuthenticator missing_transport(nullptr);
  NativeAuthSession session;
  const auto missing_transport_result = missing_transport.authenticate(
      NativeAuthRequest{options()}, &session);
  ok = expect(!missing_transport_result.ok,
              "missing transport should fail validation") &&
       ok;
  ok = expect(missing_transport_result.code == "transport_missing",
              "missing transport should use transport_missing") &&
       ok;

  FakeProtocolTransport transport;
  transport.auth_result.ok = true;
  transport.auth_result.cookie = "webvpn=SESSION";
  NativeAuthenticator authenticator(&transport);
  const auto missing_output_result =
      authenticator.authenticate(NativeAuthRequest{options()}, nullptr);
  ok = expect(!missing_output_result.ok,
              "missing output should fail validation") &&
       ok;
  ok = expect(transport.authenticate_calls == 0,
              "missing output should not call transport authenticate") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = fake_transport_success_populates_session() && ok;
  ok = auth_failed_propagates_exact_error_without_cstp() && ok;
  ok = auth_protocol_mismatch_propagates_exact_code() && ok;
  ok = plaintext_password_is_not_added_to_diagnostics_or_errors() && ok;
  ok = missing_transport_or_output_is_rejected() && ok;

  return ok ? 0 : 1;
}
