#include "vpn_engine/protocol/production_transport.hpp"

#include "vpn_engine/protocol/cstp.hpp"

#include <cstdint>
#include <deque>
#include <iostream>
#include <limits>
#include <optional>
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

std::string token_from_cookie_fixture(const std::string &cookie) {
  const std::size_t eq = cookie.find('=');
  if (eq == std::string::npos)
    return cookie;
  const std::size_t semi = cookie.find(';', eq + 1);
  return cookie.substr(eq + 1, semi == std::string::npos ? std::string::npos
                                                         : semi - eq - 1);
}

std::string aggregate_auth_http_response(const std::string &body) {
  return http_response(200, "OK",
                       {"Content-Type: text/xml; charset=utf-8",
                        "Cache-Control: no-store"},
                       body);
}

std::string aggregate_auth_http_response_with_headers(
    const std::vector<std::string> &headers,
    const std::string &body) {
  std::vector<std::string> all_headers{
      "Content-Type: text/xml; charset=utf-8",
      "Cache-Control: no-store"};
  all_headers.insert(all_headers.end(), headers.begin(), headers.end());
  return http_response(200, "OK", all_headers, body);
}

// Hex representation of a chunk size, lowercase, no leading zeros (rfc7230).
std::string hex_chunk_size(std::size_t value) {
  if (value == 0) return std::string("0");
  std::string out;
  while (value > 0) {
    char c = "0123456789abcdef"[value & 0xF];
    out.insert(out.begin(), c);
    value >>= 4;
  }
  return out;
}

// Raw HTTP/1.1 response framed with Transfer-Encoding: chunked. Each entry of
// `chunks` becomes one chunk; the trailing "0\r\n\r\n" terminator is appended
// automatically. Used by aggregate-auth framing tests.
std::string chunked_xml_response(const std::vector<std::string> &chunks) {
  std::string out = "HTTP/1.1 200 OK\r\n";
  out += "Content-Type: text/xml; charset=utf-8\r\n";
  out += "Transfer-Encoding: chunked\r\n";
  out += "\r\n";
  for (const std::string &chunk : chunks) {
    out += hex_chunk_size(chunk.size());
    out += "\r\n";
    out += chunk;
    out += "\r\n";
  }
  out += "0\r\n\r\n";
  return out;
}

std::string chunked_xml_response_with_headers_and_trailers(
    const std::vector<std::string> &headers,
    const std::vector<std::string> &chunks,
    const std::vector<std::string> &trailers) {
  std::string out = "HTTP/1.1 200 OK\r\n";
  out += "Content-Type: text/xml; charset=utf-8\r\n";
  out += "Transfer-Encoding: chunked\r\n";
  for (const std::string &header : headers) {
    out += header;
    out += "\r\n";
  }
  out += "\r\n";
  for (const std::string &chunk : chunks) {
    out += hex_chunk_size(chunk.size());
    out += "\r\n";
    out += chunk;
    out += "\r\n";
  }
  out += "0\r\n";
  for (const std::string &trailer : trailers) {
    out += trailer;
    out += "\r\n";
  }
  out += "\r\n";
  return out;
}

// HTTP/1.1 response without Content-Length and without Transfer-Encoding —
// the gateway closes the TCP connection to delimit the body. Aggregate-auth
// init responses on legacy AnyConnect deployments occasionally use this
// framing; the read path must keep reading until EOF rather than treating
// an unterminated header as an empty body.
std::string close_delimited_xml_response(const std::string &body) {
  std::string out = "HTTP/1.1 200 OK\r\n";
  out += "Content-Type: text/xml; charset=utf-8\r\n";
  out += "Connection: close\r\n";
  out += "\r\n";
  out += body;
  return out;
}

std::string keep_alive_undelimited_xml_response(const std::string &body) {
  std::string out = "HTTP/1.1 200 OK\r\n";
  out += "Content-Type: text/xml; charset=utf-8\r\n";
  out += "Connection: keep-alive\r\n";
  out += "\r\n";
  out += body;
  return out;
}

// HTTP/1.1 200 OK with Content-Length: 0 — gateway accepted the request but
// returned no body. Should NOT be parsed as XML (would surface as a generic
// "aggregate auth response is empty"); the auth call site must short-circuit
// to auth_protocol_mismatch with framing diagnostics.
std::string empty_aggregate_auth_response() {
  return "HTTP/1.1 200 OK\r\n"
         "Content-Type: text/xml; charset=utf-8\r\n"
         "Content-Length: 0\r\n"
         "\r\n";
}

const char *aggregate_auth_error_body() {
  return "<config-auth client=\"vpn\" type=\"error\">"
         "<error>backend rejected the credentials for tests</error>"
         "</config-auth>";
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
    ++read_count_;
    if (!out)
      return invalid("tls_stream_null_output", "read output must not be null");
    if (!connected_)
      return invalid("tls_stream_not_connected", "TLS stream is not connected");
    if (closed_)
      return invalid("tls_stream_closed", "TLS stream is closed");

    out->clear();
    if (read_chunks_.empty()) {
      if (exhausted_read_failure_) {
        return *exhausted_read_failure_;
      }
      return {};
    }

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

  void fail_when_reads_exhausted(
      ecnuvpn::vpn_engine::ValidationResult failure) {
    exhausted_read_failure_ = std::move(failure);
  }

  int connect_count() const { return connect_count_; }
  int close_count() const { return close_count_; }
  int write_count() const { return write_count_; }
  int read_count() const { return read_count_; }
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
  std::optional<ecnuvpn::vpn_engine::ValidationResult> exhausted_read_failure_;
  bool connected_ = false;
  bool closed_ = false;
  int connect_count_ = 0;
  int close_count_ = 0;
  int write_count_ = 0;
  int read_count_ = 0;
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
  (void)cookie;
  return aggregate_auth_http_response(
      "<config-auth client=\"vpn\" type=\"auth-request\">"
      "<auth id=\"main\"><form>"
      "<input type=\"text\" name=\"username\" label=\"Username\" />"
      "<input type=\"password\" name=\"password\" label=\"Password\" />"
      "<input type=\"hidden\" name=\"group_list\" value=\"students\" />"
      "</form></auth>"
      "<opaque>OPAQUE_ONE</opaque>"
      "</config-auth>");
}

std::string login_get_group_select() {
  return aggregate_auth_http_response(
      "<config-auth client=\"vpn\" type=\"auth-request\">"
      "<auth id=\"main\"><message>Select VPN group.</message><form>"
      "<select name=\"group_list\" label=\"VPN group\">"
      "<option value=\"students\">Students</option>"
      "<option value=\"staff\">Faculty and staff</option>"
      "</select>"
      "</form></auth>"
      "<opaque>OPAQUE_ONE</opaque>"
      "</config-auth>");
}

std::string login_get_selected_group_without_value() {
  return aggregate_auth_http_response(
      "<config-auth client=\"vpn\" type=\"auth-request\" "
      "aggregate-auth-version=\"2\">"
      "<auth id=\"main\"><form>"
      "<input type=\"text\" name=\"username\" label=\"Username:\" />"
      "<input type=\"password\" name=\"password\" label=\"Password:\" />"
      "<select name=\"group_list\" label=\"GROUP:\">"
      "<option selected=\"true\">ECNU</option>"
      "</select>"
      "</form></auth>"
      "<opaque>OPAQUE_ONE</opaque>"
      "</config-auth>");
}

std::string login_post_ok(const std::string &cookie) {
  return aggregate_auth_http_response(
      "<config-auth client=\"vpn\" type=\"complete\">"
      "<auth id=\"success\"><message>Login successful</message></auth>"
      "<session-token>" +
      token_from_cookie_fixture(cookie) +
      "</session-token>"
      "<opaque>OPAQUE_ONE</opaque>"
      "</config-auth>");
}

std::string login_post_failed() {
  return aggregate_auth_http_response(
      "<config-auth client=\"vpn\" type=\"auth-reply\">"
      "<auth id=\"error\"><message>invalid username or password</message></auth>"
      "<error>invalid username or password</error>"
      "</config-auth>");
}

std::string login_post_challenge() {
  return aggregate_auth_http_response(
      "<config-auth client=\"vpn\" type=\"auth-reply\">"
      "<auth id=\"main\"><message>Enter verification code</message><form>"
      "<input type=\"password\" name=\"secondary_password\" "
      "label=\"Verification code\" />"
      "</form></auth>"
      "<opaque>OPAQUE_TWO</opaque>"
      "</config-auth>");
}

std::string login_post_host_scan_required() {
  return aggregate_auth_http_response(
      "<config-auth client=\"vpn\" type=\"auth-reply\">"
      "<auth id=\"main\"><message>Host scan required</message></auth>"
      "<host-scan ticket=\"CSD_TICKET_SEED\" token=\"CSD_TOKEN_SEED\" "
      "base-uri=\"/+CSCOE+/sdesktop/\" "
      "wait-uri=\"/+CSCOE+/sdesktop/wait.html\" />"
      "</config-auth>");
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
  ok = expect(auth.cookie == "webvpn=SESSION",
              "auth should map session-token to webvpn cookie") &&
       ok;
  ok = expect(stream.connect_count() == 1, "authenticate should open TLS once") && ok;
  ok = expect(stream.last_endpoint().host == "vpn.example.invalid",
              "authenticate should use server host") &&
       ok;
  ok = expect(stream.last_endpoint().sni_host == "vpn.example.invalid",
              "authenticate should use host as SNI") &&
       ok;
  ok = expect(stream.writes().size() == 2,
              "authenticate should send aggregate-auth init and auth-reply") &&
       ok;

  const std::string init_request = as_text(stream.writes()[0]);
  const std::string post_request = as_text(stream.writes()[1]);
  ok = expect(contains(init_request, "POST / HTTP/1.1\r\n"),
              "auth init should POST to gateway root") &&
       ok;
  ok = expect(contains(init_request, "Content-Type: application/xml; charset=utf-8\r\n"),
              "auth init should use XML content type") &&
       ok;
  ok = expect(contains(init_request, "Accept-Encoding: identity\r\n"),
              "auth init should request identity encoding") &&
       ok;
  ok = expect(contains(init_request, "X-Transcend-Version: 1\r\n"),
              "auth init should send transcend header") &&
       ok;
  ok = expect(contains(init_request, "X-Aggregate-Auth: 1\r\n"),
              "auth init should send aggregate-auth header") &&
       ok;
  ok = expect(contains(init_request,
                       "<config-auth client=\"vpn\" type=\"init\" "
                       "aggregate-auth-version=\"2\">"),
              "auth init should send aggregate-auth v2 init XML") &&
       ok;
  ok = expect(contains(post_request, "POST / HTTP/1.1\r\n"),
              "auth reply should POST to gateway root") &&
       ok;
  ok = expect(contains(post_request,
                       "<config-auth client=\"vpn\" type=\"auth-reply\" "
                       "aggregate-auth-version=\"2\">"),
              "auth reply should send aggregate-auth v2 XML") &&
       ok;
  ok = expect(contains(post_request, "<opaque>OPAQUE_ONE</opaque>"),
              "auth reply should echo opaque XML") &&
       ok;
  ok = expect(contains(post_request,
                       "<username>student@example.invalid</username>"),
              "auth reply should include username as direct auth child") &&
       ok;
  ok = expect(contains(post_request,
                       "<password>test-mock pass%!</password>"),
              "auth reply should include password as direct auth child") &&
       ok;
  ok = expect(!contains(post_request, "<form>") &&
                  !contains(post_request, "<input"),
              "auth reply should not wrap direct credentials in form inputs") &&
       ok;
  ok = expect(!contains(init_request, "/+CSCOE+/logon.html") &&
                  !contains(post_request, "/+CSCOE+/logon.html"),
              "aggregate-auth flow must not use legacy HTML login path") &&
       ok;
  ok = expect(!contains(post_request,
                        "application/x-www-form-urlencoded") &&
                  !contains(post_request, "username=student%40example.invalid"),
              "auth reply must not send form-urlencoded credentials") &&
       ok;

  return ok;
}

bool challenge_handler_can_continue_aggregate_auth() {
  using ecnuvpn::vpn_engine::protocol::AuthInteractionRequest;
  using ecnuvpn::vpn_engine::protocol::AuthInteractionResponse;
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_challenge());
  stream.push_read_text(login_post_ok("webvpn_session=SESSION_AFTER_2FA"));

  auto opts = options();
  int handler_calls = 0;
  opts.auth_interaction_handler =
      [&](const AuthInteractionRequest &request) -> AuthInteractionResponse {
    ++handler_calls;
    ok = expect(request.kind == "challenge",
                "challenge handler should receive challenge kind") && ok;
    ok = expect(request.label == "Verification code",
                "challenge handler should receive prompt label") && ok;
    ok = expect(request.input_type == "password",
                "challenge handler should receive prompt input type") && ok;
    AuthInteractionResponse response;
    response.ok = true;
    response.value = "654321";
    return response;
  };

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(opts);

  ok = expect(auth.ok, "challenge continuation should authenticate") && ok;
  ok = expect(auth.cookie == "webvpn=SESSION_AFTER_2FA",
              "challenge continuation should return final webvpn cookie") &&
       ok;
  ok = expect(handler_calls == 1,
              "challenge continuation should prompt exactly once") &&
       ok;
  ok = expect(stream.writes().size() == 3,
              "challenge continuation should send one follow-up auth-reply") &&
       ok;

  const std::string challenge_reply = as_text(stream.writes()[2]);
  ok = expect(contains(challenge_reply, "654321"),
              "challenge follow-up should include handler response") &&
       ok;
  ok = expect(contains(challenge_reply, "<opaque>OPAQUE_TWO</opaque>"),
              "challenge follow-up should echo challenge opaque XML") &&
       ok;
  ok = expect(!contains(challenge_reply, MOCK_PASSWORD_SPECIAL),
              "challenge follow-up should not resend the primary password") &&
       ok;

  return ok;
}

bool configured_auth_group_answers_group_select_without_prompt() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_group_select());
  stream.push_read_text(login_post_ok("webvpn_session=SESSION"));

  auto opts = options();
  opts.auth_group = "staff";

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(opts);

  ok = expect(auth.ok, "configured auth group should authenticate") && ok;
  ok = expect(auth.cookie == "webvpn=SESSION",
              "configured auth group should return final cookie") &&
       ok;
  ok = expect(stream.writes().size() == 2,
              "configured auth group should not require follow-up prompt") &&
       ok;

  const std::string auth_reply = as_text(stream.writes()[1]);
  ok = expect(contains(auth_reply, "<group-select>staff</group-select>"),
              "auth reply should include configured auth group as group-select") &&
       ok;

  return ok;
}

bool selected_group_text_without_value_is_sent_in_auth_reply() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_selected_group_without_value());
  stream.push_read_text(login_post_ok("webvpn_session=SESSION"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(auth.ok,
              "selected group text without value should authenticate") &&
       ok;
  ok = expect(auth.cookie == "webvpn=SESSION",
              "selected group text without value should return final cookie") &&
       ok;
  ok = expect(stream.writes().size() == 2,
              "selected group text should complete with one auth-reply") &&
       ok;

  const std::string auth_reply = as_text(stream.writes()[1]);
  ok = expect(contains(auth_reply, "<group-select>ECNU</group-select>"),
              "auth reply should send selected option text as group-select") &&
       ok;
  return ok;
}

bool bad_credentials_return_auth_rejected_without_secret_text() {
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
  ok = expect(auth.error_code == "auth_rejected",
              "bad credentials should return auth_rejected") &&
       ok;
  ok = expect(auth.error_message.find(opts.password) == std::string::npos,
              "auth error message must not include password") &&
       ok;

  return ok;
}

bool host_scan_required_returns_unsupported_without_secret_text() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_host_scan_required());

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok, "host-scan response should fail authentication") && ok;
  ok = expect(auth.error_code == "csd_required_unsupported",
              "host-scan response should return csd_required_unsupported") &&
       ok;
  ok = expect(auth.error_message.find("CSD_TICKET_SEED") == std::string::npos,
              "host-scan auth error must not include ticket value") &&
       ok;
  ok = expect(auth.error_message.find("CSD_TOKEN_SEED") == std::string::npos,
              "host-scan auth error must not include token value") &&
       ok;

  return ok;
}

bool missing_cookie_is_protocol_error() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(aggregate_auth_http_response(
      "<config-auth client=\"vpn\" type=\"complete\">"
      "<auth id=\"success\"><message>Login successful</message></auth>"
      "</config-auth>"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok, "missing cookie should fail authentication") && ok;
  ok = expect(auth.error_code == "protocol_error",
              "missing cookie should be a protocol error") &&
       ok;

  return ok;
}

bool post_set_cookie_session_satisfies_tokenless_success() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(aggregate_auth_http_response_with_headers(
      {"Set-Cookie: webvpn_session=SESSION_FROM_HEADER; Path=/; Secure; HttpOnly"},
      "<config-auth client=\"vpn\" type=\"complete\">"
      "<auth id=\"success\"><message>Login successful</message></auth>"
      "</config-auth>"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(auth.ok,
              "tokenless aggregate-auth success with post Set-Cookie should "
              "satisfy authentication") &&
       ok;
  ok = expect(auth.cookie == "webvpn=SESSION_FROM_HEADER",
              "post Set-Cookie webvpn_session should normalize to webvpn "
              "cookie header") &&
       ok;
  return ok;
}

bool preflight_cookie_without_post_session_is_protocol_error() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok("webvpn_prelogin=PRELOGIN"));
  stream.push_read_text(aggregate_auth_http_response(
      "<config-auth client=\"vpn\" type=\"complete\">"
      "<auth id=\"success\"><message>Login successful</message></auth>"
      "</config-auth>"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok,
              "tokenless success must not satisfy authentication") &&
       ok;
  ok = expect(auth.error_code == "protocol_error",
              "missing POST session cookie should be a protocol error") &&
       ok;
  ok = expect(auth.error_message == "missing session token in aggregate-auth response",
              "transport should return parser's missing-token error") &&
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
  ok = expect(contains(connect_request, "CONNECT /CSCOSSLC/tunnel HTTP/1.1\r\n"),
              "CSTP should use AnyConnect v2 tunnel path") &&
       ok;
  ok = expect(!contains(connect_request, "CONNECT /CSCOT/ HTTP/1.1\r\n"),
              "CSTP must not use legacy CSCOT path") &&
       ok;
  ok = expect(contains(connect_request, "Cookie: webvpn=SESSION\r\n"),
              "CSTP should send auth cookie") &&
       ok;
  ok = expect(contains(connect_request, "User-Agent: ECNU-VPN native test\r\n"),
              "CSTP should use configured User-Agent") &&
       ok;
  ok = expect(contains(connect_request, "X-CSTP-Version: 1\r\n"),
              "CSTP should send version header") &&
       ok;
  ok = expect(contains(connect_request, "X-CSTP-Address-Type: IPv6,IPv4\r\n"),
              "CSTP should request IPv6 and IPv4") &&
       ok;
  ok = expect(contains(connect_request, "X-CSTP-Base-MTU: 1290\r\n"),
              "CSTP should send configured base MTU") &&
       ok;
  ok = expect(contains(connect_request, "X-CSTP-MTU: 1290\r\n"),
              "CSTP should send configured MTU") &&
       ok;
  ok = expect(contains(connect_request, "X-CSTP-Accept-Encoding: identity\r\n"),
              "CSTP should request identity payload encoding") &&
       ok;
  ok = expect(contains(connect_request, "X-Transcend-Version: 1\r\n"),
              "CSTP should send transcend header") &&
       ok;
  ok = expect(contains(connect_request, "X-Aggregate-Auth: 1\r\n"),
              "CSTP should send aggregate-auth header") &&
       ok;
  ok = expect(!contains(connect_request, "X-DTLS-"),
              "CSTP-only request should not advertise DTLS headers") &&
       ok;
  ok = expect(metadata.internal_ip4_address == "10.255.0.10",
              "CSTP metadata should include tunnel address") &&
       ok;
  ok = expect(metadata.mtu == 1400, "CSTP metadata should include MTU") && ok;
  ok = expect(metadata.routes.size() == 2,
              "CSTP metadata should include split routes") &&
       ok;
  ok = expect(metadata.dtls_state == "disabled",
              "CSTP metadata should expose disabled DTLS state") &&
       ok;
  ok = expect(metadata.dtls_fallback_reason ==
                  "DTLS disabled by native engine policy",
              "CSTP metadata should explain DTLS policy fallback") &&
       ok;

  return ok;
}

bool connect_cstp_with_dtls_enabled_still_avoids_unimplemented_headers() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_ok("webvpn_session=SESSION"));
  stream.push_read_text(cstp_connect_ok());

  auto opts = options();
  opts.disable_dtls = false;

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(opts);
  ok = expect(auth.ok, "auth should succeed before DTLS-enabled CSTP connect") &&
       ok;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  auto connected = transport.connect_cstp(auth.cookie, &metadata);

  ok = expect(connected.ok, "DTLS-enabled CSTP connect should still succeed") &&
       ok;
  const std::string connect_request = as_text(stream.writes()[2]);
  ok = expect(!contains(connect_request, "X-DTLS-"),
              "DTLS-enabled pre-A14 request must not advertise DTLS headers") &&
       ok;
  ok = expect(metadata.dtls_state == "disabled",
              "DTLS-enabled CSTP without gateway advert should remain TLS") &&
       ok;
  ok = expect(metadata.dtls_fallback_reason ==
                  "gateway did not advertise DTLS",
              "DTLS-enabled CSTP should record missing gateway advert") &&
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

bool cstp_401_maps_to_auth_expired() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(login_get_ok());
  stream.push_read_text(login_post_ok("webvpn_session=SECRET_COOKIE"));
  stream.push_read_text(http_response(401, "Unauthorized",
                                      {"Content-Type: text/plain; charset=utf-8"},
                                      "expired"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());
  ok = expect(auth.ok, "auth should succeed before expired CSTP cookie") && ok;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  auto connected = transport.connect_cstp(auth.cookie, &metadata);

  ok = expect(!connected.ok, "expired CSTP cookie should fail") && ok;
  ok = expect(connected.code == "auth_expired",
              "CSTP 401 should map to auth_expired") &&
       ok;
  ok = expect(connected.message.find("SECRET_COOKIE") == std::string::npos,
              "auth_expired message must not include cookie") &&
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
  ok = expect(first.cookie == "webvpn=OLD_COOKIE",
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
  ok = expect(second.cookie == "webvpn=NEW_COOKIE",
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
  stream.push_read_text(login_get_ok());

  auto opts = options();
  opts.password = MOCK_PASSWORD;
  stream.fail_write(2,
                    invalid("tls_write_failed",
                            "request failed: password=test-mock-password-placeholder, "
                            "password=test-mock-password-placeholder"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(opts);

  ok = expect(!auth.ok, "write error should fail authentication") && ok;
  ok = expect(auth.error_message.find(MOCK_PASSWORD) == std::string::npos,
              "write error should redact raw password") &&
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

// ---------------------------------------------------------------------------
// HTTP body framing — chunked, close-delimited, empty-Content-Length.
//
// These tests pin the read_http_response() contract that aggregate-auth
// depends on. Without them, a gateway that returns Transfer-Encoding: chunked
// or close-delimited bodies surfaces to the user as "aggregate auth response
// is empty", and an empty 200 OK is mis-classified as a credential failure.
// See docs/AGGREGATE_AUTH_EMPTY_RESPONSE_FIX_PLAN.md §3.
// ---------------------------------------------------------------------------

bool chunked_aggregate_auth_response_split_across_tls_reads() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  // Single chunked response, but pushed in two halves so the read path
  // exercises read_more() between the chunk-size line and the chunk body.
  const std::string body = aggregate_auth_error_body();
  const std::string framed = chunked_xml_response({body.substr(0, 30),
                                                   body.substr(30)});
  // Split the framed response after the headers so the chunked decoder has
  // to call read_more() to fetch the second chunk.
  const std::size_t header_split =
      framed.find("\r\n\r\n") + 4 /* through delimiter */ +
      hex_chunk_size(30).size() + 2 /* "\r\n" */ + 30 + 2 /* chunk + CRLF */;
  MockTlsStream stream;
  stream.push_read_text(framed.substr(0, header_split));
  stream.push_read_text(framed.substr(header_split));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok,
              "chunked-error response should fail authenticate") && ok;
  ok = expect(auth.error_code == "auth_rejected",
              "chunked-error body must decode through chunk framing and "
              "produce auth_rejected, not a generic empty-response error") &&
       ok;
  ok = expect(auth.error_message.find("0\r\n") == std::string::npos,
              "auth error message must not leak the chunked terminator") &&
       ok;
  return ok;
}

bool chunked_aggregate_auth_response_in_single_tls_read() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  // Same response, but headers + every chunk arrive in one TLS read. The
  // decoder must still strip chunk-size lines from the body so the XML
  // parser does not see "1c\r\n<config-auth..." as garbage.
  const std::string body = aggregate_auth_error_body();
  const std::string framed = chunked_xml_response({body.substr(0, 28),
                                                   body.substr(28, 28),
                                                   body.substr(56)});

  MockTlsStream stream;
  stream.push_read_text(framed);

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok,
              "single-read chunked-error should fail authenticate") && ok;
  ok = expect(auth.error_code == "auth_rejected",
              "single-read chunked-error must produce auth_rejected, "
              "proving chunk-size framing was stripped from body") &&
       ok;
  return ok;
}

bool chunked_aggregate_auth_request_with_trailers_preserves_next_response() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  const std::string init_response = login_get_ok();
  const std::size_t body_at = init_response.find("\r\n\r\n");
  const std::string init_body =
      body_at == std::string::npos ? std::string()
                                   : init_response.substr(body_at + 4);
  const std::string framed_init =
      chunked_xml_response_with_headers_and_trailers(
          {"Trailer: X-Gateway-Debug, X-Gateway-Trace"},
          {init_body.substr(0, 31), init_body.substr(31)},
          {"X-Gateway-Debug: ignored", "X-Gateway-Trace: ignored"});

  MockTlsStream stream;
  stream.push_read_text(framed_init + login_post_ok("webvpn_session=SESSION"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(auth.ok,
              "chunked auth-request trailers must be fully consumed so the "
              "next aggregate-auth response can be parsed") &&
       ok;
  ok = expect(auth.cookie == "webvpn=SESSION",
              "chunked auth-request with trailers should complete auth") &&
       ok;
  return ok;
}

bool chunked_aggregate_auth_ignores_malformed_content_length() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  const std::string body = aggregate_auth_error_body();
  const std::string framed =
      chunked_xml_response_with_headers_and_trailers({"Content-Length: abc"},
                                                     {body}, {});

  MockTlsStream stream;
  stream.push_read_text(framed);

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok,
              "chunked error response with malformed Content-Length should "
              "still fail authentication") &&
       ok;
  ok = expect(auth.error_code == "auth_rejected",
              "Transfer-Encoding: chunked must take precedence over a "
              "malformed Content-Length header") &&
       ok;
  return ok;
}

bool connection_close_aggregate_auth_response_reads_until_eof() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  // No Content-Length, no Transfer-Encoding, Connection: close. The body is
  // delivered in two TLS reads, then the stream EOFs. The read path must
  // accumulate the body across reads and treat EOF as the body terminator
  // instead of returning the partial buffer or a transport_closed error.
  const std::string body = aggregate_auth_error_body();
  const std::string framed = close_delimited_xml_response(body);
  const std::size_t mid = framed.find("\r\n\r\n") + 4 + body.size() / 2;

  MockTlsStream stream;
  stream.push_read_text(framed.substr(0, mid));
  stream.push_read_text(framed.substr(mid));
  // Empty read_chunks_ now → next read_some returns empty out → EOF.

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok,
              "close-delimited error response should fail authenticate") &&
       ok;
  ok = expect(auth.error_code == "auth_rejected",
              "close-delimited body must be read until EOF and parsed, "
              "producing auth_rejected rather than transport_closed or "
              "auth_protocol_mismatch") &&
       ok;
  return ok;
}

bool keep_alive_undelimited_aggregate_auth_response_fails_fast() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  MockTlsStream stream;
  stream.push_read_text(
      keep_alive_undelimited_xml_response(aggregate_auth_error_body()));
  stream.fail_when_reads_exhausted(
      invalid("tls_read_timeout", "mock TLS read would block waiting for EOF"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok,
              "keep-alive response without Content-Length or chunked framing "
              "must fail auth") &&
       ok;
  ok = expect(auth.error_code == "auth_protocol_mismatch",
              "keep-alive response without a body delimiter must be reported "
              "as an auth protocol mismatch, not as a transport timeout") &&
       ok;
  ok = expect(stream.read_count() == 1,
              "keep-alive response without a body delimiter must fail before "
              "attempting another TLS read") &&
       ok;
  return ok;
}

bool content_length_zero_aggregate_auth_response_reports_protocol_mismatch() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  bool ok = true;
  // 200 OK with Content-Length: 0 — body is empty. The auth call site must
  // detect this BEFORE feeding "" to the XML parser and emit a protocol
  // mismatch error carrying status / content-type / content-length /
  // transfer-encoding / body_bytes for operators to triage from logs.
  MockTlsStream stream;
  stream.push_read_text(empty_aggregate_auth_response());

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  ok = expect(!auth.ok,
              "Content-Length: 0 must fail authenticate") && ok;
  ok = expect(auth.error_code == "auth_protocol_mismatch",
              "Content-Length: 0 must surface as auth_protocol_mismatch, "
              "not auth_failed or auth_response_invalid leaking through "
              "the keyword fallback") &&
       ok;
  ok = expect(auth.error_message.find("status=200") != std::string::npos,
              "auth_protocol_mismatch message must include status= for "
              "operator triage") &&
       ok;
  ok = expect(auth.error_message.find("content-length=0") != std::string::npos,
              "auth_protocol_mismatch message must include content-length=") &&
       ok;
  ok = expect(auth.error_message.find("body_bytes=0") != std::string::npos,
              "auth_protocol_mismatch message must include body_bytes=") &&
       ok;
  ok = expect(auth.error_message.find(MOCK_PASSWORD_SPECIAL) == std::string::npos,
              "auth_protocol_mismatch message must not leak the password") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = authenticate_success_sends_login_get_post_and_returns_cookie() && ok;
  ok = challenge_handler_can_continue_aggregate_auth() && ok;
  ok = configured_auth_group_answers_group_select_without_prompt() && ok;
  ok = selected_group_text_without_value_is_sent_in_auth_reply() && ok;
  ok = bad_credentials_return_auth_rejected_without_secret_text() && ok;
  ok = host_scan_required_returns_unsupported_without_secret_text() && ok;
  ok = missing_cookie_is_protocol_error() && ok;
  ok = post_set_cookie_session_satisfies_tokenless_success() && ok;
  ok = preflight_cookie_without_post_session_is_protocol_error() && ok;
  ok = connect_cstp_sends_connect_and_parses_metadata() && ok;
  ok = connect_cstp_with_dtls_enabled_still_avoids_unimplemented_headers() && ok;
  ok = cstp_non_2xx_fails_without_cookie_text() && ok;
  ok = cstp_401_maps_to_auth_expired() && ok;
  ok = oversized_http_header_fails_before_unbounded_read() && ok;
  ok = oversized_http_body_fails_before_body_read() && ok;
  ok = overflowing_http_content_length_fails_before_target_calculation() && ok;
  ok = failed_cstp_connect_body_is_not_reused_by_retry() && ok;
  ok = failed_cstp_connect_read_clears_stale_bytes_before_retry() && ok;
  ok = exchange_packet_writes_data_frame_and_reads_partial_inbound_frame() && ok;
  ok = eof_during_cstp_exchange_returns_transport_closed() && ok;
  // HTTP body framing — see docs/AGGREGATE_AUTH_EMPTY_RESPONSE_FIX_PLAN.md §3.
  ok = chunked_aggregate_auth_response_split_across_tls_reads() && ok;
  ok = chunked_aggregate_auth_response_in_single_tls_read() && ok;
  ok = chunked_aggregate_auth_request_with_trailers_preserves_next_response() &&
       ok;
  ok = chunked_aggregate_auth_ignores_malformed_content_length() && ok;
  ok = connection_close_aggregate_auth_response_reads_until_eof() && ok;
  ok = keep_alive_undelimited_aggregate_auth_response_fails_fast() && ok;
  ok = content_length_zero_aggregate_auth_response_reports_protocol_mismatch() && ok;
  ok = reset_for_reconnect_closes_stream_and_clears_cookie_state() && ok;
  ok = write_errors_redact_password_and_cookie_values() && ok;
  ok = disconnect_sends_best_effort_disconnect_frame_and_closes_stream() && ok;

  return ok ? 0 : 1;
}
