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

  // Encode keepalive frame.
  {
    CstpFrame f;
    f.type = CstpFrameType::keepalive;
    f.payload.clear();

    std::vector<std::uint8_t> out;
    auto r = encode_cstp_frame(f, &out);
    ok = expect(r.ok, "keepalive frame should encode") && ok;

    // [type=1][len=0]
    ok = expect(out == bytes({1, 0, 0, 0, 0}), "keepalive encoding should match") && ok;
  }

  // Decode data frame.
  {
    const std::vector<std::uint8_t> payload = {0x01, 0x02, 0x03, 0x04};
    const std::vector<std::uint8_t> wire = bytes({0, 0, 0, 0, 4, 0x01, 0x02, 0x03, 0x04});

    CstpFrame f;
    auto r = decode_cstp_frame(wire, &f);
    ok = expect(r.ok, "data frame should decode") && ok;
    ok = expect(f.type == CstpFrameType::data, "decoded type should be data") && ok;
    ok = expect(f.payload == payload, "decoded payload should match") && ok;
  }

  // Concatenated frames: decode first then decode second.
  {
    const std::vector<std::uint8_t> frame1 = bytes({1, 0, 0, 0, 0}); // keepalive
    const std::vector<std::uint8_t> frame2 = bytes({0, 0, 0, 0, 2, 0xAA, 0xBB}); // data

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
    const std::vector<std::uint8_t> wire = bytes({1, 0, 0, 0, 0, 0xFF});
    ByteReader reader(wire);
    CstpFrame f;
    auto r = decode_cstp_frame(&reader, &f);
    ok = expect(r.ok, "frame should decode even with trailing bytes") && ok;
    ok = expect(f.type == CstpFrameType::keepalive, "decoded type should be keepalive") && ok;
    ok = expect(reader.remaining() == 1, "trailing byte should remain") && ok;
  }

  // Partial frame fails with stable code and must not consume bytes.
  {
    // Claims payload_len=3 but only provides 2 bytes.
    const std::vector<std::uint8_t> wire = bytes({0, 0, 0, 0, 3, 0xAA, 0xBB});
    ByteReader reader(wire);
    CstpFrame f;
    auto r = decode_cstp_frame(&reader, &f);
    ok = expect(!r.ok && r.code == "cstp_frame_incomplete",
                "partial frame must be rejected deterministically") && ok;
    ok = expect(reader.position() == 0, "reader position must not advance on incomplete") && ok;
    ok = expect(reader.remaining() == wire.size(), "reader remaining must be unchanged") && ok;
  }

  // Reject oversized frames.
  {
    // payload_len = 0x0010_0001 (1 MiB + 1)
    const std::vector<std::uint8_t> wire = bytes({0, 0, 0x10, 0x00, 0x01});
    CstpFrame f;
    auto r = decode_cstp_frame(wire, &f);
    ok = expect(!r.ok && r.code == "cstp_frame_oversized",
                "oversized payload must be rejected deterministically") && ok;
  }

  return ok ? 0 : 1;
}
