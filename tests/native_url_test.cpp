#include "vpn_engine/protocol/http.hpp"
#include "vpn_engine/protocol/url.hpp"

#include <iostream>
#include <string>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

} // namespace

int main() {
  using exv::vpn_engine::protocol::HttpResponse;
  using exv::vpn_engine::protocol::ParsedVpnUrl;
  using exv::vpn_engine::protocol::parse_http_response;
  using exv::vpn_engine::protocol::parse_vpn_url;

  bool ok = true;

  {
    ParsedVpnUrl parsed;
    auto res = parse_vpn_url("vpn.ecnu.edu.cn", &parsed);
    ok = expect(res.ok, "host-only URL should parse") && ok;
    ok = expect(parsed.scheme == "https", "default scheme should be https") && ok;
    ok = expect(parsed.host == "vpn.ecnu.edu.cn", "host should be preserved") && ok;
    ok = expect(parsed.port == 443, "default port should be 443") && ok;
    ok = expect(parsed.base_path == "/", "default base_path should be /") && ok;
  }

  {
    ParsedVpnUrl parsed;
    auto res = parse_vpn_url("https://vpn.ecnu.edu.cn", &parsed);
    ok = expect(res.ok, "https URL should parse") && ok;
    ok = expect(parsed.scheme == "https", "scheme should be https") && ok;
    ok = expect(parsed.host == "vpn.ecnu.edu.cn", "host should parse") && ok;
    ok = expect(parsed.port == 443, "https default port should be 443") && ok;
    ok = expect(parsed.base_path == "/", "missing path should default to /") && ok;
  }

  {
    ParsedVpnUrl parsed;
    auto res = parse_vpn_url("https://vpn.ecnu.edu.cn/", &parsed);
    ok = expect(res.ok, "https URL with trailing slash should parse") && ok;
    ok = expect(parsed.base_path == "/", "base_path should remain /") && ok;
  }

  {
    ParsedVpnUrl parsed;
    auto res = parse_vpn_url("http://vpn.ecnu.edu.cn", &parsed);
    ok = expect(!res.ok, "http scheme must be rejected") && ok;
  }

  {
    ParsedVpnUrl parsed;
    auto res = parse_vpn_url("", &parsed);
    ok = expect(!res.ok, "empty input must be rejected") && ok;
  }

  {
    ParsedVpnUrl parsed;
    auto res = parse_vpn_url("https://", &parsed);
    ok = expect(!res.ok, "empty host must be rejected") && ok;
  }

  {
    ParsedVpnUrl parsed;
    auto res = parse_vpn_url("https://vpn.ecnu.edu.cn:4443/", &parsed);
    ok = expect(res.ok, "explicit port should be accepted") && ok;
    ok = expect(parsed.port == 4443, "explicit port should be preserved") && ok;
  }

  {
    HttpResponse resp;
    const std::string raw =
        "HTTP/1.1 200 OK\r\n"
        "Content-Type: text/plain\r\n"
        "X-Test: Value\r\n"
        "\r\n"
        "hello";
    auto res = parse_http_response(raw, &resp);
    ok = expect(res.ok, "HTTP response should parse") && ok;
    ok = expect(resp.status == 200, "status should parse") && ok;
    ok = expect(resp.body == "hello", "body should be preserved") && ok;
    ok = expect(resp.header_ci("content-type") != nullptr,
                "header lookup should be case-insensitive") && ok;
    ok = expect(resp.header_ci("Content-Type") != nullptr,
                "header lookup should be case-insensitive") && ok;
  }

  {
    HttpResponse resp;
    auto res = parse_http_response("\r\n\r\n", &resp);
    ok = expect(!res.ok, "empty/malformed HTTP response must be rejected") && ok;
  }

  {
    HttpResponse resp;
    const std::string raw = "HTTP/1.1 000 OK\r\n\r\n";
    auto res = parse_http_response(raw, &resp);
    ok = expect(!res.ok, "HTTP status 000 must be rejected") && ok;
  }

  {
    HttpResponse resp;
    const std::string raw = "NOTHTTP 200 OK\r\n\r\n";
    auto res = parse_http_response(raw, &resp);
    ok = expect(!res.ok, "malformed status line must be rejected") && ok;
  }

  {
    HttpResponse resp;
    const std::string raw =
        "HTTP/1.1 200 OK\r\n"
        "Set-Cookie: a=1; Path=/\r\n"
        "Set-Cookie: b=2; Path=/\r\n"
        "\r\n";
    auto res = parse_http_response(raw, &resp);
    ok = expect(res.ok, "HTTP response with duplicate headers should parse") && ok;

    const auto *cookies = resp.header_values_ci("set-cookie");
    ok = expect(cookies != nullptr, "Set-Cookie values should be preserved") && ok;
    ok = expect(cookies && cookies->size() == 2,
                "duplicate Set-Cookie should preserve two values") && ok;
    ok = expect(cookies && cookies->size() >= 2 && (*cookies)[0] == "a=1; Path=/",
                "first Set-Cookie value should match") && ok;
    ok = expect(cookies && cookies->size() >= 2 && (*cookies)[1] == "b=2; Path=/",
                "second Set-Cookie value should match") && ok;
  }

  return ok ? 0 : 1;
}
