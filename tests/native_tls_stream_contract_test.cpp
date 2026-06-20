#include "vpn_engine/protocol/tls_stream.hpp"

#include <cstdint>
#include <deque>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <utility>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
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

std::vector<std::uint8_t> bytes(std::initializer_list<std::uint8_t> values) {
  return std::vector<std::uint8_t>(values.begin(), values.end());
}

class FakeTlsStream final : public exv::vpn_engine::protocol::TlsStream {
public:
  explicit FakeTlsStream(
      exv::vpn_engine::ValidationResult connect_result = {})
      : connect_result_(std::move(connect_result)) {}

  exv::vpn_engine::ValidationResult
  connect(const exv::vpn_engine::protocol::TlsEndpoint &endpoint)
      override {
    last_endpoint_ = endpoint;
    ++connect_count_;

    if (!connect_result_.ok)
      return connect_result_;

    connected_ = true;
    closed_ = false;
    return {};
  }

  exv::vpn_engine::ValidationResult
  write_all(const std::vector<std::uint8_t> &bytes) override {
    if (closed_)
      return invalid("tls_stream_closed", "TLS stream is closed");
    if (!connected_)
      return invalid("tls_stream_not_connected", "TLS stream is not connected");

    writes_.push_back(bytes);
    return {};
  }

  exv::vpn_engine::ValidationResult
  read_some(std::vector<std::uint8_t> *out) override {
    if (!out)
      return invalid("tls_stream_null_output", "read output must not be null");
    if (closed_)
      return invalid("tls_stream_closed", "TLS stream is closed");
    if (!connected_)
      return invalid("tls_stream_not_connected", "TLS stream is not connected");

    out->clear();
    if (read_chunks_.empty())
      return {};

    *out = read_chunks_.front();
    read_chunks_.pop_front();
    return {};
  }

  void close() override {
    if (closed_)
      return;

    if (connected_)
      ++close_count_;
    connected_ = false;
    closed_ = true;
  }

  void push_read(std::vector<std::uint8_t> chunk) {
    read_chunks_.push_back(std::move(chunk));
  }

  const exv::vpn_engine::protocol::TlsEndpoint &last_endpoint() const {
    return last_endpoint_;
  }

  const std::vector<std::vector<std::uint8_t>> &writes() const {
    return writes_;
  }

  int connect_count() const { return connect_count_; }
  int close_count() const { return close_count_; }
  bool connected() const { return connected_; }
  bool closed() const { return closed_; }

private:
  exv::vpn_engine::ValidationResult connect_result_;
  exv::vpn_engine::protocol::TlsEndpoint last_endpoint_;
  std::vector<std::vector<std::uint8_t>> writes_;
  std::deque<std::vector<std::uint8_t>> read_chunks_;
  bool connected_ = false;
  bool closed_ = false;
  int connect_count_ = 0;
  int close_count_ = 0;
};

bool test_endpoint_connect_contract() {
  using exv::vpn_engine::protocol::TlsEndpoint;

  bool ok = true;

  TlsEndpoint defaults;
  ok = expect(defaults.port == 443, "TLS endpoint should default to 443") && ok;

  TlsEndpoint endpoint;
  endpoint.host = "vpn.example.invalid";
  endpoint.port = 4443;
  endpoint.sni_host = "vpn-sni.example.invalid";

  FakeTlsStream stream;
  auto connected = stream.connect(endpoint);

  ok = expect(connected.ok, "connect success should be representable") && ok;
  ok = expect(stream.connect_count() == 1, "connect should be observable") && ok;
  ok = expect(stream.connected(), "successful connect should open stream") && ok;
  ok = expect(stream.last_endpoint().host == "vpn.example.invalid",
              "connect should receive endpoint host") &&
       ok;
  ok = expect(stream.last_endpoint().port == 4443,
              "connect should receive endpoint port") &&
       ok;
  ok = expect(stream.last_endpoint().sni_host == "vpn-sni.example.invalid",
              "connect should receive SNI host") &&
       ok;

  return ok;
}

bool test_write_all_partial_read_and_eof_contract() {
  using exv::vpn_engine::protocol::TlsEndpoint;

  bool ok = true;

  TlsEndpoint endpoint;
  endpoint.host = "vpn.example.invalid";
  endpoint.sni_host = endpoint.host;

  FakeTlsStream stream;
  stream.push_read(bytes({0x16, 0x03}));
  stream.push_read(bytes({0x01}));

  ok = expect(stream.connect(endpoint).ok, "connect should succeed before I/O") &&
       ok;

  auto write = stream.write_all(bytes({0x48, 0x54, 0x54, 0x50}));
  ok = expect(write.ok, "write_all success should be representable") && ok;
  ok = expect(stream.writes().size() == 1,
              "write_all should record one complete write") &&
       ok;
  ok = expect(stream.writes().size() == 1 &&
                  stream.writes()[0] == bytes({0x48, 0x54, 0x54, 0x50}),
              "write_all should preserve every byte") &&
       ok;

  std::vector<std::uint8_t> out = bytes({0xff});

  auto read1 = stream.read_some(&out);
  ok = expect(read1.ok, "first partial read should succeed") && ok;
  ok = expect(out == bytes({0x16, 0x03}),
              "first partial read should return first chunk") &&
       ok;

  auto read2 = stream.read_some(&out);
  ok = expect(read2.ok, "second partial read should succeed") && ok;
  ok = expect(out == bytes({0x01}),
              "second partial read should return second chunk") &&
       ok;

  auto eof = stream.read_some(&out);
  ok = expect(eof.ok, "EOF should be a clean read result") && ok;
  ok = expect(out.empty(), "EOF should be represented by an empty byte vector") &&
       ok;

  return ok;
}

bool test_tls_verification_failure_contract() {
  using exv::vpn_engine::protocol::TlsEndpoint;

  bool ok = true;

  FakeTlsStream stream(invalid("tls_verification_failed",
                               "certificate verification failed"));

  TlsEndpoint endpoint;
  endpoint.host = "vpn.example.invalid";
  endpoint.sni_host = endpoint.host;

  auto connected = stream.connect(endpoint);

  ok = expect(!connected.ok, "TLS verification failure should be representable") &&
       ok;
  ok = expect(connected.code == "tls_verification_failed",
              "TLS verification failure should have a stable code") &&
       ok;
  ok = expect(!stream.connected(),
              "failed TLS verification should not open stream") &&
       ok;

  auto write = stream.write_all(bytes({0x01}));
  ok = expect(!write.ok && write.code == "tls_stream_not_connected",
              "write after failed connect should fail deterministically") &&
       ok;

  stream.close();
  stream.close();
  ok = expect(stream.close_count() == 0,
              "close after failed connect should be resource-safe") &&
       ok;

  return ok;
}

bool test_idempotent_close_contract() {
  using exv::vpn_engine::protocol::TlsEndpoint;

  bool ok = true;

  TlsEndpoint endpoint;
  endpoint.host = "vpn.example.invalid";
  endpoint.sni_host = endpoint.host;

  FakeTlsStream stream;
  stream.push_read(bytes({0x17, 0x03, 0x03}));

  ok = expect(stream.connect(endpoint).ok, "connect should succeed before close") &&
       ok;

  stream.close();
  stream.close();
  stream.close();

  ok = expect(stream.closed(), "close should leave stream closed") && ok;
  ok = expect(!stream.connected(), "close should clear connected state") && ok;
  ok = expect(stream.close_count() == 1, "close should be idempotent") && ok;

  auto write = stream.write_all(bytes({0x01}));
  ok = expect(!write.ok && write.code == "tls_stream_closed",
              "write after close should fail as closed") &&
       ok;

  std::vector<std::uint8_t> out;
  auto read = stream.read_some(&out);
  ok = expect(!read.ok && read.code == "tls_stream_closed",
              "read after close should fail as closed") &&
       ok;

  return ok;
}

std::string read_file_best_effort(const std::filesystem::path &path) {
  std::ifstream file(path, std::ios::binary);
  if (!file.is_open())
    return {};

  return std::string((std::istreambuf_iterator<char>(file)),
                     std::istreambuf_iterator<char>());
}

bool has_source_extension(const std::filesystem::path &path) {
  const std::string ext = path.extension().string();
  return ext == ".cpp" || ext == ".hpp" || ext == ".h";
}

bool test_protocol_sources_do_not_include_platform_tls_headers() {
  bool ok = true;

  const std::filesystem::path protocol_dir =
      std::filesystem::path(EXV_SOURCE_DIR) / "src" / "vpn_engine" /
      "protocol";

  const std::vector<std::string> forbidden = {
      "#include <windows.h>",
      "#include <schannel.h>",
      "#include <security.h>",
      "#include <sspi.h>",
      "#include <wincrypt.h>",
      "#include <Security/Security.h>",
      "#include <Security/SecureTransport.h>",
      "#include <openssl/ssl.h>",
      "#include <openssl/err.h>",
  };

  ok = expect(std::filesystem::exists(protocol_dir),
              "protocol source directory should exist") &&
       ok;

  for (const auto &entry :
       std::filesystem::recursive_directory_iterator(protocol_dir)) {
    if (!entry.is_regular_file() || !has_source_extension(entry.path()))
      continue;

    const std::string contents = read_file_best_effort(entry.path());
    for (const std::string &needle : forbidden) {
      if (contents.find(needle) != std::string::npos) {
        std::cerr << "Forbidden include " << needle << " in " << entry.path()
                  << std::endl;
        ok = false;
      }
    }
  }

  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = test_endpoint_connect_contract() && ok;
  ok = test_write_all_partial_read_and_eof_contract() && ok;
  ok = test_tls_verification_failure_contract() && ok;
  ok = test_idempotent_close_contract() && ok;
  ok = test_protocol_sources_do_not_include_platform_tls_headers() && ok;

  return ok ? 0 : 1;
}
