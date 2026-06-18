#include "vpn_engine/protocol/production_transport.hpp"

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

ecnuvpn::vpn_engine::ValidationResult invalid(std::string code,
                                              std::string message) {
  ecnuvpn::vpn_engine::ValidationResult result;
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

class MockTlsStream final : public ecnuvpn::vpn_engine::protocol::TlsStream {
public:
  ecnuvpn::vpn_engine::ValidationResult
  connect(const ecnuvpn::vpn_engine::protocol::TlsEndpoint &) override {
    connected_ = true;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_all(const std::vector<std::uint8_t> &bytes) override {
    if (!connected_)
      return invalid("tls_stream_not_connected", "TLS stream is not connected");
    writes_.push_back(bytes);
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
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

ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions options() {
  ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions out;
  auto parsed = ecnuvpn::vpn_engine::protocol::parse_vpn_url(
      "https://vpn.example.invalid", &out.server);
  (void)parsed;
  out.username = "student";
  out.password = "password";
  out.disable_dtls = false;
  return out;
}

} // namespace

int main() {
  bool ok = true;

  MockTlsStream stream;
  stream.push_read_text(aggregate_init_ok());
  stream.push_read_text(aggregate_auth_ok());
  stream.push_read_text(cstp_with_dtls_metadata());

  ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());
  ok = expect(auth.ok, "aggregate auth should succeed") && ok;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  auto connected = transport.connect_cstp(auth.cookie, &metadata);
  ok = expect(connected.ok,
              "CSTP should remain connected when DTLS metadata is advertised") &&
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
