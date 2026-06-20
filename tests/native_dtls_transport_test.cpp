#include "vpn_engine/protocol/production_transport.hpp"
#include "vpn_engine/protocol/dtls_transport.hpp"

#include <cstdint>
#include <deque>
#include <iostream>
#include <string>
#include <utility>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

exv::vpn_engine::ValidationResult invalid(std::string code,
                                              std::string message) {
  exv::vpn_engine::ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

std::vector<std::uint8_t> text_bytes(const std::string &text) {
  return std::vector<std::uint8_t>(text.begin(), text.end());
}

std::string as_text(const std::vector<std::uint8_t> &bytes) {
  return std::string(bytes.begin(), bytes.end());
}

class MockTlsStream final : public exv::vpn_engine::protocol::TlsStream {
public:
  exv::vpn_engine::ValidationResult
  connect(const exv::vpn_engine::protocol::TlsEndpoint &) override {
    connected_ = true;
    return {};
  }

  exv::vpn_engine::ValidationResult
  write_all(const std::vector<std::uint8_t> &bytes) override {
    if (!connected_)
      return invalid("tls_stream_not_connected", "TLS stream is not connected");
    writes_.push_back(bytes);
    return {};
  }

  exv::vpn_engine::ValidationResult
  read_some(std::vector<std::uint8_t> *out) override {
    if (!out)
      return invalid("tls_stream_null_output", "read output must not be null");
    out->clear();
    if (reads_.empty())
      return {};
    *out = std::move(reads_.front());
    reads_.pop_front();
    return {};
  }

  void close() override { connected_ = false; }

  void push_read_text(const std::string &text) { reads_.push_back(text_bytes(text)); }

  const std::vector<std::vector<std::uint8_t>> &writes() const {
    return writes_;
  }

private:
  bool connected_ = false;
  std::deque<std::vector<std::uint8_t>> reads_;
  std::vector<std::vector<std::uint8_t>> writes_;
};

std::string http_response(int status, const std::string &reason,
                          const std::vector<std::string> &headers,
                          const std::string &body = {}) {
  std::string out = "HTTP/1.1 " + std::to_string(status) + " " + reason + "\r\n";
  for (const auto &header : headers) {
    out += header;
    out += "\r\n";
  }
  out += "Content-Length: " + std::to_string(body.size()) + "\r\n\r\n";
  out += body;
  return out;
}

std::string aggregate_init_ok() {
  return http_response(200, "OK", {"Content-Type: text/xml; charset=utf-8"},
                       "<config-auth client=\"vpn\" type=\"init\"/>");
}

std::string aggregate_auth_ok() {
  return http_response(200, "OK",
                       {"Content-Type: text/xml; charset=utf-8",
                        "Set-Cookie: webvpn=SESSION; Path=/; Secure"},
                       "<config-auth><session-token>SESSION</session-token>"
                       "</config-auth>");
}

std::string cstp_with_dtls_metadata() {
  return "HTTP/1.1 200 OK\r\n"
         "X-CSTP-Address: 10.255.0.10\r\n"
         "X-CSTP-Netmask: 255.255.255.0\r\n"
         "X-CSTP-MTU: 1400\r\n"
         "X-DTLS-Session-ID: advertised\r\n"
         "X-DTLS12-CipherSuite: TLS_PSK_WITH_AES_256_GCM_SHA384\r\n"
         "\r\n";
}

exv::vpn_engine::protocol::ProtocolSessionOptions options() {
  exv::vpn_engine::protocol::ProtocolSessionOptions out;
  auto parsed = exv::vpn_engine::protocol::parse_vpn_url(
      "https://vpn.example.invalid", &out.server);
  (void)parsed;
  out.username = "student";
  out.password = "MOCK_PASSWORD";
  out.disable_dtls = false;
  return out;
}

bool test_dtls_state_classification() {
  using exv::vpn_engine::protocol::DtlsNegotiationInput;
  using exv::vpn_engine::protocol::DtlsTransportState;
  using exv::vpn_engine::protocol::classify_dtls_negotiation;
  using exv::vpn_engine::protocol::dtls_transport_state_to_string;

  bool ok = true;

  DtlsNegotiationInput disabled;
  disabled.disabled_by_config = true;
  auto disabled_status = classify_dtls_negotiation(disabled);
  ok = expect(disabled_status.state == DtlsTransportState::disabled,
              "disabled config should classify DTLS as disabled") &&
       ok;
  ok = expect(std::string(dtls_transport_state_to_string(
                  disabled_status.state)) == "disabled",
              "disabled state should have stable public string") &&
       ok;

  DtlsNegotiationInput connected;
  connected.disabled_by_config = false;
  connected.gateway_advertised = true;
  connected.backend_available = true;
  connected.handshake_succeeded = true;
  auto connected_status = classify_dtls_negotiation(connected);
  ok = expect(connected_status.state ==
                  DtlsTransportState::attempted_and_connected,
              "successful backend handshake should classify as connected") &&
       ok;

  DtlsNegotiationInput fallback;
  fallback.disabled_by_config = false;
  fallback.gateway_advertised = true;
  fallback.backend_available = false;
  fallback.tls_fallback_allowed = true;
  auto fallback_status = classify_dtls_negotiation(fallback);
  ok = expect(fallback_status.state ==
                  DtlsTransportState::attempted_and_fell_back_to_tls,
              "missing backend with fallback should report TLS fallback") &&
       ok;
  ok = expect(fallback_status.cstp_tls_active,
              "fallback state should keep CSTP/TLS active") &&
       ok;

  DtlsNegotiationInput failed;
  failed.disabled_by_config = false;
  failed.gateway_advertised = true;
  failed.backend_available = true;
  failed.handshake_succeeded = false;
  failed.tls_fallback_allowed = false;
  auto failed_status = classify_dtls_negotiation(failed);
  ok = expect(failed_status.state ==
                  DtlsTransportState::attempted_and_failed_without_tls_fallback,
              "failed handshake without fallback should be explicit") &&
       ok;
  ok = expect(!failed_status.cstp_tls_active,
              "no-fallback failed state should not claim CSTP/TLS is active") &&
       ok;

  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = test_dtls_state_classification() && ok;

  MockTlsStream stream;
  stream.push_read_text(aggregate_init_ok());
  stream.push_read_text(aggregate_auth_ok());
  stream.push_read_text(cstp_with_dtls_metadata());

  exv::vpn_engine::protocol::ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());
  ok = expect(auth.ok, "aggregate auth should succeed") && ok;

  exv::vpn_engine::TunnelMetadata metadata;
  auto connected = transport.connect_cstp(auth.cookie, &metadata);
  ok = expect(connected.ok,
              "CSTP should remain connected when DTLS metadata is advertised") &&
       ok;
  ok = expect(metadata.dtls_state == "attempted_and_fell_back_to_tls",
              "advertised DTLS should report explicit CSTP/TLS fallback") &&
       ok;
  ok = expect(!metadata.dtls_fallback_reason.empty(),
              "DTLS fallback should include public reason") &&
       ok;

  const std::string connect_request = as_text(stream.writes().back());
  ok = expect(connect_request.find("X-DTLS-") == std::string::npos,
              "CSTP-only baseline must not send X-DTLS request headers") &&
       ok;
  ok = expect(connect_request.find("CONNECT /CSCOSSLC/tunnel HTTP/1.1\r\n") !=
                  std::string::npos,
              "CSTP-only baseline should use AnyConnect CSTP tunnel path") &&
       ok;

  if (ok) {
    std::cout << "native_dtls_transport_test: all assertions passed\n";
  } else {
    std::cerr << "native_dtls_transport_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
