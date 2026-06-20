#include "vpn_engine/protocol/cstp.hpp"
#include "vpn_engine/protocol/http.hpp"

#include <cstdint>
#include <filesystem>
#include <fstream>
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

std::string read_text_file_best_effort(const std::filesystem::path &p) {
  std::ifstream f(p, std::ios::binary);
  if (!f.is_open())
    return {};
  std::string s((std::istreambuf_iterator<char>(f)), std::istreambuf_iterator<char>());
  return s;
}

std::string load_fixture(const std::string &relative_under_tests) {
  namespace fs = std::filesystem;

  const fs::path rel = fs::path(relative_under_tests);

  // Primary: derive from __FILE__ (typically absolute in CMake builds).
  {
    fs::path base = fs::path(__FILE__).parent_path();
    fs::path candidate = base / rel;
    std::string s = read_text_file_best_effort(candidate);
    if (!s.empty())
      return s;
  }

  // Fallback: try walking up from current directory.
  fs::path cwd = fs::current_path();
  for (int i = 0; i < 6; ++i) {
    fs::path candidate = cwd / "tests" / rel;
    std::string s = read_text_file_best_effort(candidate);
    if (!s.empty())
      return s;
    if (!cwd.has_parent_path())
      break;
    cwd = cwd.parent_path();
  }

  return {};
}

std::vector<std::uint8_t> bytes(std::initializer_list<std::uint8_t> b) {
  return std::vector<std::uint8_t>(b.begin(), b.end());
}

} // namespace

int main() {
  using ecnuvpn::vpn_engine::TunnelMetadata;
  using ecnuvpn::vpn_engine::protocol::CstpFrame;
  using ecnuvpn::vpn_engine::protocol::CstpFrameType;
  using ecnuvpn::vpn_engine::protocol::ByteReader;
  using ecnuvpn::vpn_engine::protocol::HttpResponse;
  using ecnuvpn::vpn_engine::protocol::decode_cstp_frame;
  using ecnuvpn::vpn_engine::protocol::encode_cstp_frame;
  using ecnuvpn::vpn_engine::protocol::parse_cstp_headers;
  using ecnuvpn::vpn_engine::protocol::parse_http_response;

  bool ok = true;

  // Header parsing (success fixture).
  {
    const std::string raw = load_fixture("fixtures/native_anyconnect/cstp_connect_success.http");
    ok = expect(!raw.empty(), "success fixture should be loadable") && ok;

    HttpResponse resp;
    auto r = parse_http_response(raw, &resp);
    ok = expect(r.ok, "fixture HTTP should parse") && ok;

    TunnelMetadata meta;
    auto c = parse_cstp_headers(resp, &meta);
    ok = expect(c.ok, "CSTP headers should parse") && ok;
    ok = expect(meta.internal_ip4_address == "10.255.0.10", "IPv4 address should parse") && ok;
    ok = expect(meta.internal_ip4_netmask == "255.255.255.0", "IPv4 netmask should parse") && ok;
    ok = expect(meta.mtu == 1400, "MTU should parse") && ok;

    ok = expect(meta.routes.size() == 2, "split include routes should parse") && ok;
    ok = expect(meta.routes.size() >= 1 && meta.routes[0] == "198.51.100.0/24",
                "first split include should match") && ok;
    ok = expect(meta.routes.size() >= 2 && meta.routes[1] == "203.0.113.0/24",
                "second split include should match") && ok;

    ok = expect(meta.server_bypass_ips.size() == 1, "bypass routes should parse") && ok;
    ok = expect(meta.server_bypass_ips.size() >= 1 && meta.server_bypass_ips[0] == "192.0.2.0/24",
                "bypass route should match") && ok;
  }

  // Header parsing (failure fixture should be rejected deterministically).
  {
    const std::string raw = load_fixture("fixtures/native_anyconnect/cstp_connect_failure.http");
    ok = expect(!raw.empty(), "failure fixture should be loadable") && ok;

    HttpResponse resp;
    auto r = parse_http_response(raw, &resp);
    ok = expect(r.ok, "failure fixture HTTP should parse") && ok;

    TunnelMetadata meta;
    auto c = parse_cstp_headers(resp, &meta);
    ok = expect(!c.ok && c.code == "cstp_connect_failed",
                "non-2xx CSTP response should fail with stable code") && ok;
  }

  // Header parsing (AnyConnect v2 metadata fixture).
  {
    const std::string raw =
        load_fixture("fixtures/native_anyconnect_v2/cstp_connect_success.http");
    ok = expect(!raw.empty(), "v2 CSTP fixture should be loadable") && ok;

    HttpResponse resp;
    auto r = parse_http_response(raw, &resp);
    ok = expect(r.ok, "v2 fixture HTTP should parse") && ok;

    TunnelMetadata meta;
    auto c = parse_cstp_headers(resp, &meta);
    ok = expect(c.ok, "v2 CSTP headers should parse") && ok;
    ok = expect(meta.ip6_address == "2001:db8:100::10",
                "IPv6 address should parse") &&
         ok;
    ok = expect(meta.ip6_prefix == 64, "IPv6 prefix should parse") && ok;
    ok = expect(meta.dns_servers.size() == 2 &&
                    meta.dns_servers[0] == "10.10.10.10" &&
                    meta.dns_servers[1] == "10.10.10.11",
                "repeated DNS headers should parse") &&
         ok;
    ok = expect(meta.nbns_servers.size() == 1 &&
                    meta.nbns_servers[0] == "10.10.20.10",
                "NBNS headers should parse") &&
         ok;
    ok = expect(meta.default_domain == "ecnu.example.invalid",
                "default domain should parse") &&
         ok;
    ok = expect(meta.search_domains.size() == 1 &&
                    meta.search_domains[0] == "campus.example.invalid",
                "split DNS/search domain should parse") &&
         ok;
    ok = expect(meta.split_include_routes.size() == 1 &&
                    meta.split_include_routes[0] == "198.51.100.0/24",
                "split include routes should parse into explicit field") &&
         ok;
    ok = expect(meta.routes == meta.split_include_routes,
                "legacy routes field should mirror split include routes") &&
         ok;
    ok = expect(meta.split_exclude_routes.size() == 1 &&
                    meta.split_exclude_routes[0] == "203.0.113.0/24",
                "split exclude routes should parse") &&
         ok;
    ok = expect(meta.tunnel_all_dns, "tunnel-all-DNS should parse") && ok;
    ok = expect(meta.banner == "Welcome student",
                "URL-encoded CSTP banner should decode") &&
         ok;
    ok = expect(meta.keepalive_seconds == 20, "keepalive should parse") && ok;
    ok = expect(meta.dpd_seconds == 30, "DPD should parse") && ok;
    ok = expect(meta.rekey_seconds == 3600, "rekey time should parse") && ok;
    ok = expect(meta.rekey_method == "new-tunnel",
                "rekey method should parse") &&
         ok;
    ok = expect(meta.lease_duration_seconds == 7200,
                "lease duration should parse") &&
         ok;
    ok = expect(meta.idle_timeout_seconds == 1800,
                "idle timeout should parse") &&
         ok;
    ok = expect(meta.session_timeout_seconds == 28800,
                "session timeout should parse") &&
         ok;
    ok = expect(meta.disconnected_timeout_seconds == 60,
                "disconnected timeout should parse") &&
         ok;
    ok = expect(meta.dtls_mtu == 1360, "DTLS MTU should parse") && ok;
    ok = expect(meta.dtls_port == 443, "DTLS port should parse") && ok;
    ok = expect(meta.dtls_session_id == "DTLS_SESSION",
                "DTLS session id should parse") &&
         ok;
    ok = expect(meta.dtls_cipher_suite == "AES256-SHA",
                "legacy DTLS cipher should parse") &&
         ok;
    ok = expect(meta.dtls12_cipher_suite ==
                    "TLS_PSK_WITH_AES_256_GCM_SHA384",
                "DTLS 1.2 cipher should parse") &&
         ok;
    ok = expect(meta.content_encoding == "lzs",
                "CSTP content encoding should parse") &&
         ok;
  }

  // Optional timeout metadata may be advisory strings on some gateways.
  {
    const std::string raw =
        "HTTP/1.1 200 OK\r\n"
        "X-CSTP-Address: 10.255.0.10\r\n"
        "X-CSTP-Netmask: 255.255.255.0\r\n"
        "X-CSTP-MTU: 1400\r\n"
        "X-CSTP-Idle-Timeout: disabled\r\n"
        "X-CSTP-Session-Timeout: none\r\n"
        "X-CSTP-Disconnected-Timeout: none\r\n"
        "\r\n";

    HttpResponse resp;
    auto r = parse_http_response(raw, &resp);
    ok = expect(r.ok, "non-numeric timeout HTTP should parse") && ok;

    TunnelMetadata meta;
    auto c = parse_cstp_headers(resp, &meta);
    ok = expect(c.ok, "non-numeric optional timeout headers should not fail CSTP") &&
         ok;
    ok = expect(meta.idle_timeout_seconds == 0,
                "ignored idle timeout should keep default") &&
         ok;
    ok = expect(meta.session_timeout_seconds == 0,
                "ignored session timeout should keep default") &&
         ok;
    ok = expect(meta.disconnected_timeout_seconds == 0,
                "ignored disconnected timeout should keep default") &&
         ok;
  }

  // Strict optional transport numbers should still reject malformed values.
  {
    const std::string raw =
        "HTTP/1.1 200 OK\r\n"
        "X-CSTP-Address: 10.255.0.10\r\n"
        "X-CSTP-Netmask: 255.255.255.0\r\n"
        "X-CSTP-MTU: 1400\r\n"
        "X-DTLS-MTU: nope\r\n"
        "\r\n";

    HttpResponse resp;
    auto r = parse_http_response(raw, &resp);
    ok = expect(r.ok, "malformed strict optional HTTP should parse") && ok;

    TunnelMetadata meta;
    auto c = parse_cstp_headers(resp, &meta);
    ok = expect(!c.ok && c.code == "cstp_invalid_number",
                "malformed DTLS MTU should remain a strict CSTP error") &&
         ok;
  }

  // Encode keepalive frame.
  {
    CstpFrame f;
    f.type = CstpFrameType::keepalive;
    f.payload.clear();

    std::vector<std::uint8_t> out;
    auto r = encode_cstp_frame(f, &out);
    ok = expect(r.ok, "keepalive frame should encode") && ok;

    // STF header: 'S''T''F' 0x01 | len_be16=0 | type=0x07 | 0x00
    ok = expect(out == bytes({0x53, 0x54, 0x46, 0x01, 0x00, 0x00, 0x07, 0x00}),
                "keepalive encoding should match STF framing") &&
         ok;
  }

  // Byte-exact encode vectors for every frame type (empty payload).
  {
    struct Case {
      CstpFrameType type;
      std::uint8_t tag;
      const char *name;
    };
    const Case cases[] = {
        {CstpFrameType::data, 0x00, "data"},
        {CstpFrameType::dpd_request, 0x03, "dpd_request"},
        {CstpFrameType::dpd_response, 0x04, "dpd_response"},
        {CstpFrameType::disconnect, 0x05, "disconnect"},
        {CstpFrameType::keepalive, 0x07, "keepalive"},
        {CstpFrameType::compressed, 0x08, "compressed"},
        {CstpFrameType::terminate, 0x09, "terminate"},
    };
    for (const Case &c : cases) {
      CstpFrame f;
      f.type = c.type;
      std::vector<std::uint8_t> out;
      auto r = encode_cstp_frame(f, &out);
      ok = expect(r.ok, "frame type should encode") && ok;
      ok = expect(out == bytes({0x53, 0x54, 0x46, 0x01, 0x00, 0x00, c.tag, 0x00}),
                  c.name) &&
           ok;

      // Round-trip decode restores the type.
      CstpFrame decoded;
      auto d = decode_cstp_frame(out, &decoded);
      ok = expect(d.ok && decoded.type == c.type, "frame type round-trips") && ok;
    }
  }

  // Decode data frame.
  {
    const std::vector<std::uint8_t> payload = {0x01, 0x02, 0x03, 0x04};
    const std::vector<std::uint8_t> wire =
        bytes({0x53, 0x54, 0x46, 0x01, 0x00, 0x04, 0x00, 0x00, 0x01, 0x02, 0x03,
               0x04});

    CstpFrame f;
    auto r = decode_cstp_frame(wire, &f);
    ok = expect(r.ok, "data frame should decode") && ok;
    ok = expect(f.type == CstpFrameType::data, "decoded type should be data") && ok;
    ok = expect(f.payload == payload, "decoded payload should match") && ok;
  }

  // Concatenated frames: decode first then decode second.
  {
    const std::vector<std::uint8_t> frame1 =
        bytes({0x53, 0x54, 0x46, 0x01, 0x00, 0x00, 0x07, 0x00}); // keepalive
    const std::vector<std::uint8_t> frame2 = bytes(
        {0x53, 0x54, 0x46, 0x01, 0x00, 0x02, 0x00, 0x00, 0xAA, 0xBB}); // data

    std::vector<std::uint8_t> wire;
    wire.insert(wire.end(), frame1.begin(), frame1.end());
    wire.insert(wire.end(), frame2.begin(), frame2.end());

    ByteReader reader(wire);

    CstpFrame a;
    auto r1 = decode_cstp_frame(&reader, &a);
    ok = expect(r1.ok, "first concatenated frame should decode") && ok;
    ok = expect(a.type == CstpFrameType::keepalive, "first frame type should match") && ok;
    ok = expect(reader.remaining() == frame2.size(), "reader should retain remaining bytes") && ok;

    CstpFrame b;
    auto r2 = decode_cstp_frame(&reader, &b);
    ok = expect(r2.ok, "second concatenated frame should decode") && ok;
    ok = expect(b.type == CstpFrameType::data, "second frame type should match") && ok;
    ok = expect(b.payload == bytes({0xAA, 0xBB}), "second frame payload should match") && ok;
    ok = expect(reader.remaining() == 0, "reader should be fully consumed") && ok;
  }

  // First frame with trailing bytes should be accepted by ByteReader decode.
  {
    const std::vector<std::uint8_t> wire =
        bytes({0x53, 0x54, 0x46, 0x01, 0x00, 0x00, 0x07, 0x00, 0xFF});
    ByteReader reader(wire);
    CstpFrame f;
    auto r = decode_cstp_frame(&reader, &f);
    ok = expect(r.ok, "frame should decode even with trailing bytes") && ok;
    ok = expect(f.type == CstpFrameType::keepalive, "decoded type should be keepalive") && ok;
    ok = expect(reader.remaining() == 1, "trailing byte should remain") && ok;
  }

  // Partial payload fails with stable code and must not consume bytes.
  {
    // Header claims payload_len=3 but only provides 2 payload bytes.
    const std::vector<std::uint8_t> wire =
        bytes({0x53, 0x54, 0x46, 0x01, 0x00, 0x03, 0x00, 0x00, 0xAA, 0xBB});
    ByteReader reader(wire);
    CstpFrame f;
    auto r = decode_cstp_frame(&reader, &f);
    ok = expect(!r.ok && r.code == "cstp_frame_incomplete",
                "partial payload must be rejected deterministically") && ok;
    ok = expect(reader.position() == 0, "reader position must not advance on incomplete") && ok;
    ok = expect(reader.remaining() == wire.size(), "reader remaining must be unchanged") && ok;
  }

  // Partial header (fewer than 8 header bytes) is incomplete, not an error.
  {
    const std::vector<std::uint8_t> wire = bytes({0x53, 0x54, 0x46, 0x01, 0x00});
    ByteReader reader(wire);
    CstpFrame f;
    auto r = decode_cstp_frame(&reader, &f);
    ok = expect(!r.ok && r.code == "cstp_frame_incomplete",
                "partial header must be reported as incomplete") && ok;
    ok = expect(reader.position() == 0, "reader must not advance on partial header") && ok;
  }

  // Bad magic is a hard error (not incomplete), so callers stop reading.
  {
    const std::vector<std::uint8_t> wire =
        bytes({0x00, 0x54, 0x46, 0x01, 0x00, 0x00, 0x07, 0x00});
    ByteReader reader(wire);
    CstpFrame f;
    auto r = decode_cstp_frame(&reader, &f);
    ok = expect(!r.ok && r.code == "cstp_bad_magic",
                "bad magic must fail deterministically") && ok;
    ok = expect(reader.position() == 0, "reader must not advance on bad magic") && ok;
  }

  // Reject oversized frames at encode time (payload exceeds uint16 length).
  {
    CstpFrame f;
    f.type = CstpFrameType::data;
    f.payload.assign(0x10000, 0x00); // 65536 bytes, one past uint16 max
    std::vector<std::uint8_t> out;
    auto r = encode_cstp_frame(f, &out);
    ok = expect(!r.ok && r.code == "cstp_frame_oversized",
                "oversized payload must be rejected deterministically") && ok;
  }

  return ok ? 0 : 1;
}
