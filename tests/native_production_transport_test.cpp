#include "vpn_engine/protocol/production_transport.hpp"

#include "vpn_engine/protocol/cstp.hpp"

#include <cstdint>
#include <deque>
#include <iostream>
#include <limits>
#include <string>
#include <utility>
#include <vector>

namespace {

// General-purpose mock password for tests.
static const char *MOCK_PASSWORD = "test-mock-password-placeholder";
// Mock password with special characters for percent-encoding tests.
static const char *MOCK_PASSWORD_SPECIAL = "test-mock pass%!";

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
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

std::vector<std::uint8_t> bytes(std::initializer_list<std::uint8_t> values) {
  return std::vector<std::uint8_t>(values.begin(), values.end());
}

std::vector<std::uint8_t> text_bytes(const std::string &text) {
  return std::vector<std::uint8_t>(text.begin(), text.end());
}

std::string as_text(const std::vector<std::uint8_t> &bytes) {
  return std::string(bytes.begin(), bytes.end());
}

bool contains(const std::string &haystack, const std::string &needle) {
  return haystack.find(needle) != std::string::npos;
}

std::string http_response(int status, const std::string &reason,
                          const std::vector<std::string> &headers,
                          const std::string &body = "") {
  std::string out = "HTTP/1.1 " + std::to_string(status) + " " + reason + "\r\n";
  for (const std::string &header : headers) {
    out += header;
    out += "\r\n";
  }
  out += "Content-Length: ";
  out += std::to_string(body.size());
  out += "\r\n\r\n";
  out += body;
  return out;
}

std::vector<std::uint8_t> encoded_frame(
    ecnuvpn::vpn_engine::protocol::CstpFrameType type,
    std::vector<std::uint8_t> payload = {}) {
  ecnuvpn::vpn_engine::protocol::CstpFrame frame;
  frame.type = type;
  frame.payload = std::move(payload);

  std::vector<std::uint8_t> out;
  auto encoded = ecnuvpn::vpn_engine::protocol::encode_cstp_frame(frame, &out);
  if (!encoded.ok)
    return {};
  return out;
}

class MockTlsStream final : public ecnuvpn::vpn_engine::protocol::TlsStream {
public:
  ecnuvpn::vpn_engine::ValidationResult
  connect(const ecnuvpn::vpn_engine::protocol::TlsEndpoint &endpoint) override {
    last_endpoint_ = endpoint;
    ++connect_count_;
    if (!connect_result_.ok)
      return connect_result_;
    connected_ = true;
    closed_ = false;
    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  write_all(const std::vector<std::uint8_t> &bytes) override {
    ++write_count_;
    writes_.push_back(bytes);

    if (fail_write_number_ == write_count_)
      return write_failure_;
    if (!connected_)
      return invalid("tls_stream_not_connected", "TLS stream is not connected");
    if (closed_)
      return invalid("tls_stream_closed", "TLS stream is closed");

    return {};
  }

  ecnuvpn::vpn_engine::ValidationResult
  read_some(std::vector<std::uint8_t> *out) override {
    if (!out)
      return invalid("tls_stream_null_output", "read output must not be null");
    if (!connected_)
      return invalid("tls_stream_not_connected", "TLS stream is not connected");
    if (closed_)
      return invalid("tls_stream_closed", "TLS stream is closed");

    out->clear();
    if (read_chunks_.empty())
      return {};

    *out = std::move(read_chunks_.front());
    read_chunks_.pop_front();
    return {};
  }

  void close() override {
    if (connected_)
      ++close_count_;
    connected_ = false;
    closed_ = true;
  }

  void push_read(std::vector<std::uint8_t> chunk) {
    read_chunks_.push_back(std::move(chunk));
  }

  void push_read_text(const std::string &text) { push_read(text_bytes(text)); }

  void fail_write(int write_number,
                  ecnuvpn::vpn_engine::ValidationResult failure) {
    fail_write_number_ = write_number;
    write_failure_ = std::move(failure);
  }

  int connect_count() const { return connect_count_; }
  int close_count() const { return close_count_; }
  int write_count() const { return write_count_; }
  bool closed() const { return closed_; }

  const ecnuvpn::vpn_engine::protocol::TlsEndpoint &last_endpoint() const {
    return last_endpoint_;
  }

  const std::vector<std::vector<std::uint8_t>> &writes() const {
    return writes_;
  }

private:
  ecnuvpn::vpn_engine::ValidationResult connect_result_;
  ecnuvpn::vpn_engine::ValidationResult write_failure_;
  ecnuvpn::vpn_engine::protocol::TlsEndpoint last_endpoint_;
  std::deque<std::vector<std::uint8_t>> read_chunks_;
  std::vector<std::vector<std::uint8_t>> writes_;
  bool connected_ = false;
  bool closed_ = false;
  int connect_count_ = 0;
  int close_count_ = 0;
  int write_count_ = 0;
  int fail_write_number_ = 0;
};

ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions options() {
  ecnuvpn::vpn_engine::protocol::ProtocolSessionOptions out;
  out.server.scheme = "https";
  out.server.host = "vpn.example.invalid";
  out.server.port = 443;
  out.server.base_path = "/";
  out.username = "student@example.invalid";
  out.password = MOCK_PASSWORD_SPECIAL;
  out.useragent = "ECNU-VPN native test";
  return out;
}

std::string login_get_ok(const std::string &cookie = "") {
  std::vector<std::string> headers = {"Content-Type: text/html; charset=utf-8"};
  if (!cookie.empty())
    headers.push_back("Set-Cookie: " + cookie + "; Path=/; Secure; HttpOnly");
  return http_response(200, "OK", headers);
}

std::string login_post_ok(const std::string &cookie) {
  return http_response(
      200, "OK",
      {"Content-Type: text/html; charset=utf-8",
       "Set-Cookie: " + cookie + "; Path=/; Secure; HttpOnly"},
      "<html><body>Login OK</body></html>");
}

std::string login_post_failed() {
  return http_response(401, "Unauthorized",
                       {"Content-Type: application/json; charset=utf-8",
                        "Cache-Control: no-store"},
                       "{\"error\":\"auth_failed\",\"message\":\"invalid username or password\"}");
}

std::string cstp_connect_ok() {
  return "HTTP/1.1 200 OK\r\n"
         "X-CSTP-Address: 10.255.0.10\r\n"
         "X-CSTP-Netmask: 255.255.255.0\r\n"
         "X-CSTP-MTU: 1400\r\n"
         "X-CSTP-Split-Include: 198.51.100.0/24\r\n"
         "X-CSTP-Split-Include: 203.0.113.0/24\r\n"
         "X-CSTP-Bypass-Route: 192.0.2.0/24\r\n"
         "\r\n";
}

bool authenticate_success_sends_login_get_post_and_returns_cookie() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok("webvpn_prelogin=PRELOGIN"));
  stream.push_read_text(login_post_ok("webvpn_session=SESSION"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(auth.ok, "authentication should succeed") && ok;
  ok = expect(auth.cookie == "webvpn_prelogin=PRELOGIN; webvpn_session=SESSION",
              "auth should return stable combined cookie header") &&
       ok;
  ok = expect(stream.connect_count() == 1, "authenticate should open TLS once") && ok;
  ok = expect(stream.last_endpoint().host == "vpn.example.invalid",
              "authenticate should use server host") &&
       ok;
  ok = expect(stream.last_endpoint().sni_host == "vpn.example.invalid",
              "authenticate should use host as SNI") &&
       ok;
  ok = expect(stream.writes().size() == 2,
              "authenticate should send GET and POST") &&
       ok;

  const std::string get_request = as_text(stream.writes()[0]);
  const std::string post_request = as_text(stream.writes()[1]);
  ok = expect(contains(get_request, "GET /+CSCOE+/logon.html HTTP/1.1\r\n"),
              "login preflight should use v1 GET path") &&
       ok;
  ok = expect(contains(post_request, "POST /+CSCOE+/logon.html HTTP/1.1\r\n"),
              "login submit should use v1 POST path") &&
       ok;
  ok = expect(contains(post_request, "Content-Type: application/x-www-form-urlencoded; charset=utf-8\r\n"),
              "login submit should use form content type") &&
       ok;
  ok = expect(contains(post_request, "Cookie: webvpn_prelogin=PRELOGIN\r\n"),
              "login submit should send preflight cookie") &&
       ok;
  ok = expect(contains(post_request,
                       "\r\n\r\nusername=student%40example.invalid&password=test-mock+pass%25%21"),
              "login submit should percent-encode credentials") &&
       ok;

  return ok;
}

bool bad_credentials_return_auth_failed_without_secret_text() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_failed());

  auto opts = options();
  opts.password = MOCK_PASSWORD;

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(opts);

  ok = expect(!auth.ok, "bad credentials should fail authentication") && ok;
  ok = expect(auth.error_code == "auth_failed",
              "bad credentials should return auth_failed") &&
       ok;
  ok = expect(auth.error_message.find(opts.password) == std::string::npos,
              "auth error message must not include password") &&
       ok;

  return ok;
}

bool missing_cookie_is_protocol_error() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(http_response(200, "OK",
                                      {"Content-Type: text/html; charset=utf-8"},
                                      "<html><body>Login OK</body></html>"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok, "missing cookie should fail authentication") && ok;
  ok = expect(auth.error_code == "protocol_error",
              "missing cookie should be a protocol error") &&
       ok;

  return ok;
}

bool preflight_cookie_without_post_session_is_protocol_error() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok("webvpn_prelogin=PRELOGIN"));
  stream.push_read_text(http_response(200, "OK",
                                      {"Content-Type: text/html; charset=utf-8"},
                                      "<html><body>Login OK</body></html>"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok,
              "preflight-only cookie must not satisfy authentication") &&
       ok;
  ok = expect(auth.error_code == "protocol_error",
              "missing POST session cookie should be a protocol error") &&
       ok;
  ok = expect(auth.error_message == "missing Set-Cookie in auth response",
              "transport should return parser's missing-cookie error") &&
       ok;
  ok = expect(stream.writes().size() == 2,
              "authenticate should stop after failed POST response") &&
       ok;

  return ok;
}

bool connect_cstp_sends_connect_and_parses_metadata() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_ok("webvpn_session=SESSION"));
  stream.push_read_text(cstp_connect_ok());

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());
  ok = expect(auth.ok, "auth should succeed before CSTP connect") && ok;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  auto connected = transport.connect_cstp(auth.cookie, &metadata);

  ok = expect(connected.ok, "CSTP connect should succeed") && ok;
  ok = expect(stream.writes().size() == 3,
              "CSTP connect should add one CONNECT write") &&
       ok;
  const std::string connect_request = as_text(stream.writes()[2]);
  ok = expect(contains(connect_request, "CONNECT /CSCOT/ HTTP/1.1\r\n"),
              "CSTP should use CONNECT /CSCOT/") &&
       ok;
  ok = expect(contains(connect_request, "Cookie: webvpn_session=SESSION\r\n"),
              "CSTP should send auth cookie") &&
       ok;
  ok = expect(contains(connect_request, "X-CSTP-Version: 1\r\n"),
              "CSTP should send version header") &&
       ok;
  ok = expect(contains(connect_request, "X-CSTP-Address-Type: IPv4\r\n"),
              "CSTP should request IPv4") &&
       ok;
  ok = expect(metadata.internal_ip4_address == "10.255.0.10",
              "CSTP metadata should include tunnel address") &&
       ok;
  ok = expect(metadata.mtu == 1400, "CSTP metadata should include MTU") && ok;
  ok = expect(metadata.routes.size() == 2,
              "CSTP metadata should include split routes") &&
       ok;

  return ok;
}

bool cstp_non_2xx_fails_without_cookie_text() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_ok("webvpn_session=SECRET_COOKIE"));
  stream.push_read_text(http_response(403, "Forbidden",
                                      {"Content-Type: text/plain; charset=utf-8"},
                                      "CSTP CONNECT rejected"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());
  ok = expect(auth.ok, "auth should succeed before rejected CSTP") && ok;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  auto connected = transport.connect_cstp(auth.cookie, &metadata);

  ok = expect(!connected.ok, "non-2xx CSTP should fail") && ok;
  ok = expect(connected.code == "cstp_connect_failed",
              "non-2xx CSTP should return cstp_connect_failed") &&
       ok;
  ok = expect(connected.message.find("SECRET_COOKIE") == std::string::npos,
              "CSTP error message must not include cookie") &&
       ok;

  return ok;
}

bool oversized_http_header_fails_before_unbounded_read() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text("HTTP/1.1 200 OK\r\nX-Oversized: " +
                        std::string(70 * 1024, 'a'));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok, "oversized HTTP header should fail authentication") && ok;
  ok = expect(auth.error_code == "http_header_too_large",
              "oversized HTTP header should return http_header_too_large") &&
       ok;

  return ok;
}

bool oversized_http_body_fails_before_body_read() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text("HTTP/1.1 200 OK\r\nContent-Length: 16777217\r\n\r\n");

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok, "oversized HTTP body should fail authentication") && ok;
  ok = expect(auth.error_code == "http_body_too_large",
              "oversized HTTP body should return http_body_too_large") &&
       ok;

  return ok;
}

bool overflowing_http_content_length_fails_before_target_calculation() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text("HTTP/1.1 200 OK\r\nContent-Length: " +
                        std::to_string(std::numeric_limits<std::size_t>::max()) +
                        "\r\n\r\n");

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok, "overflowing Content-Length should fail authentication") &&
       ok;
  ok = expect(auth.error_code == "http_content_length_overflow",
              "overflowing Content-Length should return http_content_length_overflow") &&
       ok;

  return ok;
}

bool failed_cstp_connect_body_is_not_reused_by_retry() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_ok("webvpn_session=SESSION"));
  stream.push_read_text(http_response(403, "Forbidden",
                                      {"Content-Type: text/plain; charset=utf-8"},
                                      "CSTP CONNECT rejected"));
  stream.push_read_text(cstp_connect_ok());

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());
  ok = expect(auth.ok, "auth should succeed before rejected CSTP retry") && ok;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  auto rejected = transport.connect_cstp(auth.cookie, &metadata);
  ok = expect(!rejected.ok, "first CSTP connect should fail") && ok;
  ok = expect(rejected.code == "cstp_connect_failed",
              "first CSTP connect should fail with cstp_connect_failed") &&
       ok;

  auto retried = transport.connect_cstp(auth.cookie, &metadata);
  ok = expect(retried.ok,
              "retry should parse the next CONNECT response, not stale body bytes") &&
       ok;
  ok = expect(stream.writes().size() == 4,
              "retry should send a second CONNECT request") &&
       ok;
  ok = expect(metadata.internal_ip4_address == "10.255.0.10",
              "retry should populate CSTP metadata from the successful response") &&
       ok;

  return ok;
}

bool failed_cstp_connect_read_clears_stale_bytes_before_retry() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_ok("webvpn_session=SESSION"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());
  ok = expect(auth.ok, "auth should succeed before failed CSTP read retry") &&
       ok;

  stream.push_read_text("CSTP CONNECT rejected body bytes");

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  auto failed = transport.connect_cstp(auth.cookie, &metadata);
  ok = expect(!failed.ok, "incomplete CSTP CONNECT response should fail") && ok;
  ok = expect(failed.code == "transport_closed",
              "incomplete CSTP CONNECT response should fail on read EOF") &&
       ok;

  stream.push_read_text(cstp_connect_ok());

  auto retried = transport.connect_cstp(auth.cookie, &metadata);
  ok = expect(retried.ok,
              "retry should parse fresh CONNECT response after read failure") &&
       ok;
  ok = expect(stream.writes().size() == 4,
              "retry after read failure should send a second CONNECT request") &&
       ok;
  ok = expect(metadata.internal_ip4_address == "10.255.0.10",
              "retry after read failure should not parse stale buffered bytes") &&
       ok;

  return ok;
}

bool exchange_packet_writes_data_frame_and_reads_partial_inbound_frame() {
  using ecnuvpn::vpn_engine::protocol::CstpFrame;
  using ecnuvpn::vpn_engine::protocol::CstpFrameType;
  using ecnuvpn::vpn_engine::protocol::InboundFrame;
  using ecnuvpn::vpn_engine::protocol::InboundFrameKind;
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;
  using ecnuvpn::vpn_engine::protocol::decode_cstp_frame;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_ok("webvpn_session=SESSION"));
  stream.push_read_text(cstp_connect_ok());

  const auto inbound = encoded_frame(CstpFrameType::data, bytes({0x45, 0x00, 0x00, 0x14}));
  stream.push_read(std::vector<std::uint8_t>(inbound.begin(), inbound.begin() + 2));
  stream.push_read(std::vector<std::uint8_t>(inbound.begin() + 2, inbound.end()));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());
  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(auth.ok, "auth should succeed before packet exchange") && ok;
  ok = expect(transport.connect_cstp(auth.cookie, &metadata).ok,
              "CSTP should succeed before packet exchange") &&
       ok;

  auto sent = transport.send_packet(bytes({0xaa, 0xbb}));
  ok = expect(sent.ok, "send_packet should succeed") && ok;

  CstpFrame outbound;
  auto decoded = decode_cstp_frame(stream.writes().back(), &outbound);
  ok = expect(decoded.ok, "outbound CSTP frame should decode") && ok;
  ok = expect(outbound.type == CstpFrameType::data,
              "outbound CSTP frame should be data") &&
       ok;
  ok = expect(outbound.payload == bytes({0xaa, 0xbb}),
              "outbound CSTP frame should contain packet bytes") &&
       ok;

  InboundFrame frame;
  auto received = transport.receive_frame(&frame);
  ok = expect(received.ok, "receive_frame should succeed") && ok;
  ok = expect(frame.kind == InboundFrameKind::data,
              "inbound frame should be data") &&
       ok;
  ok = expect(frame.payload == bytes({0x45, 0x00, 0x00, 0x14}),
              "receive_frame should reassemble partial inbound IP packet") &&
       ok;

  return ok;
}

bool eof_during_cstp_exchange_returns_transport_closed() {
  using ecnuvpn::vpn_engine::protocol::InboundFrame;
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_ok("webvpn_session=SESSION"));
  stream.push_read_text(cstp_connect_ok());

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());
  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(auth.ok, "auth should succeed before EOF exchange") && ok;
  ok = expect(transport.connect_cstp(auth.cookie, &metadata).ok,
              "CSTP should succeed before EOF exchange") &&
       ok;

  auto sent = transport.send_packet(bytes({0xaa}));
  ok = expect(sent.ok, "send_packet should succeed before EOF read") && ok;

  InboundFrame frame;
  auto received = transport.receive_frame(&frame);

  ok = expect(!received.ok, "EOF during CSTP read should fail receive") && ok;
  ok = expect(received.code == "transport_closed",
              "EOF during CSTP should return transport_closed") &&
       ok;

  return ok;
}

bool reset_for_reconnect_closes_stream_and_clears_cookie_state() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_ok("webvpn_session=OLD_COOKIE"));

  ProductionProtocolTransport transport(&stream);
  auto first = transport.authenticate(options());
  ok = expect(first.ok, "first auth should succeed") && ok;
  ok = expect(first.cookie == "webvpn_session=OLD_COOKIE",
              "first auth should expose old cookie") &&
       ok;

  transport.reset_for_reconnect();
  ok = expect(stream.close_count() == 1,
              "reset_for_reconnect should close the stream") &&
       ok;

  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_ok("webvpn_session=NEW_COOKIE"));
  stream.push_read_text(cstp_connect_ok());

  auto second = transport.authenticate(options());
  ok = expect(second.ok, "second auth should succeed after reset") && ok;
  ok = expect(stream.connect_count() == 2,
              "second auth after reset should reopen TLS") &&
       ok;
  ok = expect(second.cookie == "webvpn_session=NEW_COOKIE",
              "reset should clear old cookie state") &&
       ok;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  auto connected = transport.connect_cstp(second.cookie, &metadata);
  ok = expect(connected.ok, "CSTP should use second auth cookie") && ok;

  const std::string cstp_request = as_text(stream.writes().back());
  ok = expect(!contains(cstp_request, "OLD_COOKIE"),
              "CSTP request after reset must not include old cookie") &&
       ok;
  ok = expect(contains(cstp_request, "NEW_COOKIE"),
              "CSTP request after reset should include new cookie") &&
       ok;

  return ok;
}

bool write_errors_redact_password_and_cookie_values() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok("webvpn_session=SECRET_COOKIE"));

  auto opts = options();
  opts.password = MOCK_PASSWORD;
  stream.fail_write(2,
                    invalid("tls_write_failed",
                            "request failed: password=test-mock-password-placeholder, "
                            "password=test-mock-password-placeholder, "
                            "Cookie: webvpn_session=SECRET_COOKIE"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(opts);

  ok = expect(!auth.ok, "write error should fail authentication") && ok;
  ok = expect(auth.error_message.find(MOCK_PASSWORD) == std::string::npos,
              "write error should redact raw password") &&
       ok;
  ok = expect(auth.error_message.find("SECRET_COOKIE") == std::string::npos,
              "write error should redact cookie value") &&
       ok;

  return ok;
}

bool disconnect_sends_best_effort_disconnect_frame_and_closes_stream() {
  using ecnuvpn::vpn_engine::protocol::CstpFrame;
  using ecnuvpn::vpn_engine::protocol::CstpFrameType;
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;
  using ecnuvpn::vpn_engine::protocol::decode_cstp_frame;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_ok("webvpn_session=SESSION"));
  stream.push_read_text(cstp_connect_ok());

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());
  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  ok = expect(auth.ok, "auth should succeed before disconnect") && ok;
  ok = expect(transport.connect_cstp(auth.cookie, &metadata).ok,
              "CSTP should succeed before disconnect") &&
       ok;

  const std::size_t writes_before_disconnect = stream.writes().size();
  transport.disconnect();

  ok = expect(stream.close_count() == 1, "disconnect should close stream") && ok;
  ok = expect(stream.writes().size() == writes_before_disconnect + 1,
              "disconnect should write one best-effort frame") &&
       ok;

  CstpFrame frame;
  auto decoded = decode_cstp_frame(stream.writes().back(), &frame);
  ok = expect(decoded.ok, "disconnect frame should decode") && ok;
  ok = expect(frame.type == CstpFrameType::disconnect,
              "disconnect frame type should be disconnect") &&
       ok;

  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = authenticate_success_sends_login_get_post_and_returns_cookie() && ok;
  ok = bad_credentials_return_auth_failed_without_secret_text() && ok;
  ok = missing_cookie_is_protocol_error() && ok;
  ok = preflight_cookie_without_post_session_is_protocol_error() && ok;
  ok = connect_cstp_sends_connect_and_parses_metadata() && ok;
  ok = cstp_non_2xx_fails_without_cookie_text() && ok;
  ok = oversized_http_header_fails_before_unbounded_read() && ok;
  ok = oversized_http_body_fails_before_body_read() && ok;
  ok = overflowing_http_content_length_fails_before_target_calculation() && ok;
  ok = failed_cstp_connect_body_is_not_reused_by_retry() && ok;
  ok = failed_cstp_connect_read_clears_stale_bytes_before_retry() && ok;
  ok = exchange_packet_writes_data_frame_and_reads_partial_inbound_frame() && ok;
  ok = eof_during_cstp_exchange_returns_transport_closed() && ok;
  ok = reset_for_reconnect_closes_stream_and_clears_cookie_state() && ok;
  ok = write_errors_redact_password_and_cookie_values() && ok;
  ok = disconnect_sends_best_effort_disconnect_frame_and_closes_stream() && ok;

  return ok ? 0 : 1;
}
