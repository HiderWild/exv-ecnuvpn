#include "vpn_engine/protocol/auth.hpp"
#include "vpn_engine/protocol/http.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <string>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

std::string read_file_bytes(const std::string &path) {
  std::ifstream in(path, std::ios::binary);
  if (!in)
    return "";
  return std::string(std::istreambuf_iterator<char>(in), std::istreambuf_iterator<char>());
}

} // namespace

int main() {
  using ecnuvpn::vpn_engine::protocol::AuthForm;
  using ecnuvpn::vpn_engine::protocol::AuthCookieJar;
  using ecnuvpn::vpn_engine::protocol::AuthResult;
  using ecnuvpn::vpn_engine::protocol::HttpResponse;
  using ecnuvpn::vpn_engine::protocol::parse_auth_form;
  using ecnuvpn::vpn_engine::protocol::parse_auth_response;
  using ecnuvpn::vpn_engine::protocol::parse_http_response;

  bool ok = true;

  {
    const std::string html =
        "<html><body>"
        "<form method=\"post\" action=\"/+webvpn+/index.html\">"
        "<input type=\"text\" name=\"username\"/>"
        "<input type=\"password\" name=\"password\"/>"
        "<input type=\"hidden\" name=\"csrf_token\" value=\"abc123\"/>"
        "</form>"
        "</body></html>";

    AuthForm form;
    auto res = parse_auth_form(html, &form);
    ok = expect(res.ok, "auth form should parse") && ok;
    ok = expect(form.action_path == "/+webvpn+/index.html", "action path should parse") && ok;
    ok = expect(form.username_field == "username", "username field should be detected") && ok;
    ok = expect(form.password_field == "password", "password field should be detected") && ok;
    ok = expect(form.hidden_fields.count("csrf_token") == 1,
                "hidden field should be collected") &&
         ok;
    ok = expect(form.hidden_fields["csrf_token"] == "abc123", "hidden field value should match") &&
         ok;
  }

  const std::filesystem::path test_dir = std::filesystem::path(__FILE__).parent_path();
  const std::filesystem::path fixture_dir = test_dir / "fixtures" / "native_anyconnect";

  {
    const std::string raw = read_file_bytes((fixture_dir / "auth_failure.http").string());
    ok = expect(!raw.empty(), "auth_failure fixture should be readable") && ok;

    HttpResponse resp;
    auto res = parse_http_response(raw, &resp);
    ok = expect(res.ok, "auth_failure HTTP response should parse") && ok;

    AuthResult ar = parse_auth_response(resp);
    ok = expect(!ar.ok, "auth failure should not be ok") && ok;
    ok = expect(ar.error_code == "auth_failed", "auth failure should map to auth_failed") && ok;
    ok = expect(ar.error_message.find("invalid") != std::string::npos,
                "auth failure message should be preserved") &&
         ok;
  }

  {
    const std::string raw = read_file_bytes((fixture_dir / "auth_success.http").string());
    ok = expect(!raw.empty(), "auth_success fixture should be readable") && ok;

    HttpResponse resp;
    auto res = parse_http_response(raw, &resp);
    ok = expect(res.ok, "auth_success HTTP response should parse") && ok;

    AuthResult ar = parse_auth_response(resp);
    ok = expect(ar.ok, "auth success should be ok") && ok;
    ok = expect(ar.cookie == "webvpn=REDACTED_COOKIE",
                "auth success should extract webvpn cookie") &&
         ok;
  }

  {
    const std::string preflight_raw =
        "HTTP/1.1 200 OK\r\n"
        "Set-Cookie: webvpn_prelogin=PRELOGIN; Path=/; Secure\r\n"
        "Set-Cookie: route=old; Path=/; Secure\r\n"
        "\r\n";
    const std::string submitted_raw =
        "HTTP/1.1 200 OK\r\n"
        "Set-Cookie: route=new; Path=/; Secure\r\n"
        "Set-Cookie: webvpn=SESSION; Path=/; Secure; HttpOnly\r\n"
        "\r\n";

    HttpResponse preflight;
    auto preflight_res = parse_http_response(preflight_raw, &preflight);
    ok = expect(preflight_res.ok, "preflight cookies HTTP response should parse") && ok;

    AuthCookieJar cookies;
    cookies.collect_from_response(preflight);
    ok = expect(cookies.header() == "webvpn_prelogin=PRELOGIN; route=old",
                "cookie jar should preserve initial Set-Cookie insertion order") &&
         ok;

    HttpResponse submitted;
    auto submitted_res = parse_http_response(submitted_raw, &submitted);
    ok = expect(submitted_res.ok, "submitted cookies HTTP response should parse") && ok;

    cookies.collect_from_response(submitted);
    ok = expect(cookies.header() ==
                    "webvpn_prelogin=PRELOGIN; route=new; webvpn=SESSION",
                "cookie jar should update existing names without moving them") &&
         ok;
  }

  {
    // Unsupported flow (e.g. SAML/SSO handoff).
    const std::string raw =
        "HTTP/1.1 200 OK\r\n"
        "Content-Type: text/html; charset=utf-8\r\n"
        "Cache-Control: no-store\r\n"
        "\r\n"
        "<html><body><form>"
        "<input type=\"hidden\" name=\"SAMLRequest\" value=\"...\"/>"
        "</form></body></html>";

    HttpResponse resp;
    auto res = parse_http_response(raw, &resp);
    ok = expect(res.ok, "unsupported-flow HTTP response should parse") && ok;

    AuthResult ar = parse_auth_response(resp);
    ok = expect(!ar.ok, "unsupported flow should not be ok") && ok;
    ok = expect(ar.error_code == "unsupported_auth_flow",
                "unsupported flow should return unsupported_auth_flow") &&
         ok;
  }

  {
    // Missing cookie is a protocol error.
    const std::string raw =
        "HTTP/1.1 200 OK\r\n"
        "Content-Type: text/html; charset=utf-8\r\n"
        "Cache-Control: no-store\r\n"
        "\r\n"
        "<html><body>Login OK</body></html>";

    HttpResponse resp;
    auto res = parse_http_response(raw, &resp);
    ok = expect(res.ok, "missing-cookie HTTP response should parse") && ok;

    AuthResult ar = parse_auth_response(resp);
    ok = expect(!ar.ok, "missing cookie should not be ok") && ok;
    ok = expect(ar.error_code == "protocol_error",
                "missing cookie should return protocol_error") &&
         ok;
  }

  return ok ? 0 : 1;
}
