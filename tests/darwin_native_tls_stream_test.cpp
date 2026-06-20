#include "platform/darwin/native_tls_stream.hpp"

#include <cstdint>
#include <deque>
#include <cerrno>
#include <iostream>
#include <memory>
#include <string>
#include <utility>
#include <vector>

#include <sys/socket.h>

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

class MockDarwinTlsApi final : public exv::platform::DarwinNativeTlsApi {
public:
  exv::platform::DarwinNativeTlsTcpConnectResult tcp_connect_result;
  exv::platform::DarwinNativeTlsHandshakeResult handshake_result;
  exv::vpn_engine::ValidationResult write_result;
  std::deque<exv::platform::DarwinNativeTlsReadResult> read_results;

  int connect_count = 0;
  int handshake_count = 0;
  int write_count = 0;
  int read_count = 0;
  int close_context_count = 0;
  int close_socket_count = 0;

  exv::vpn_engine::protocol::TlsEndpoint last_endpoint;
  std::string last_sni;
  std::vector<std::uint8_t> last_write;

  MockDarwinTlsApi() {
    tcp_connect_result.socket = 101;
    handshake_result.tls_context = 202;
  }

  exv::platform::DarwinNativeTlsTcpConnectResult
  connect_tcp(const exv::vpn_engine::protocol::TlsEndpoint &endpoint)
      override {
    ++connect_count;
    last_endpoint = endpoint;
    return tcp_connect_result;
  }

  exv::platform::DarwinNativeTlsHandshakeResult
  handshake(exv::platform::DarwinNativeTlsSocketHandle,
            const std::string &sni_host) override {
    ++handshake_count;
    last_sni = sni_host;
    return handshake_result;
  }

  exv::vpn_engine::ValidationResult
  write_plaintext(exv::platform::DarwinNativeTlsContextHandle,
                  exv::platform::DarwinNativeTlsSocketHandle,
                  const std::vector<std::uint8_t> &payload) override {
    ++write_count;
    last_write = payload;
    return write_result;
  }

  exv::platform::DarwinNativeTlsReadResult
  read_plaintext(exv::platform::DarwinNativeTlsContextHandle,
                 exv::platform::DarwinNativeTlsSocketHandle) override {
    ++read_count;
    if (read_results.empty()) {
      exv::platform::DarwinNativeTlsReadResult result;
      result.result = invalid("tls_read_failed", "no read result queued");
      return result;
    }

    exv::platform::DarwinNativeTlsReadResult result = read_results.front();
    read_results.pop_front();
    return result;
  }

  void close_tls_context(
      exv::platform::DarwinNativeTlsContextHandle) override {
    ++close_context_count;
  }

  void close_socket(
      exv::platform::DarwinNativeTlsSocketHandle) override {
    ++close_socket_count;
  }
};

std::unique_ptr<exv::platform::NativeTlsStream>
make_stream(MockDarwinTlsApi **mock) {
  auto api = std::make_unique<MockDarwinTlsApi>();
  *mock = api.get();
  return std::make_unique<exv::platform::NativeTlsStream>(std::move(api));
}

exv::vpn_engine::protocol::TlsEndpoint endpoint() {
  exv::vpn_engine::protocol::TlsEndpoint endpoint;
  endpoint.host = "vpn.example.invalid";
  endpoint.port = 4443;
  endpoint.sni_host = "sni.example.invalid";
  return endpoint;
}

struct NativeSeamState {
  int open_connected_socket_count = 0;
  int close_socket_count = 0;
  int set_socket_option_count = 0;
  int set_protocol_floor_count = 0;
  int failed_socket_option = 0;
  int last_error = EINVAL;
  int protocol_floor_result = 0;
  exv::platform::DarwinNativeTlsProtocolVersion last_protocol_floor =
      exv::platform::DarwinNativeTlsProtocolVersion::tls12;
  std::vector<int> socket_options;
};

exv::platform::DarwinNativeTlsDependencies
native_dependencies(std::shared_ptr<NativeSeamState> state) {
  exv::platform::DarwinNativeTlsDependencies deps;
  deps.tcp.open_connected_socket =
      [state](const exv::vpn_engine::protocol::TlsEndpoint &, int) {
        ++state->open_connected_socket_count;
        exv::platform::DarwinNativeTlsTcpConnectResult result;
        result.socket = 303;
        return result;
      };
  deps.tcp.close_socket = [state](
                              exv::platform::DarwinNativeTlsSocketHandle) {
    ++state->close_socket_count;
  };
  deps.socket_options.set_socket_option =
      [state](exv::platform::DarwinNativeTlsSocketHandle, int, int option,
              const void *, std::size_t) {
        ++state->set_socket_option_count;
        state->socket_options.push_back(option);
        return option == state->failed_socket_option ? -1 : 0;
      };
  deps.socket_options.last_error = [state] { return state->last_error; };
  deps.secure_transport.set_protocol_version_min =
      [state](exv::platform::DarwinNativeTlsSecureTransportContextHandle,
              exv::platform::DarwinNativeTlsProtocolVersion version) {
        ++state->set_protocol_floor_count;
        state->last_protocol_floor = version;
        return state->protocol_floor_result;
      };
  return deps;
}

bool socket_hardening_failure_fails_before_tls_handshake(int failed_option,
                                                         const char *name) {
  auto state = std::make_shared<NativeSeamState>();
  state->failed_socket_option = failed_option;
  exv::platform::NativeTlsStream stream(
      exv::platform::make_darwin_native_tls_api(
          native_dependencies(state)));

  auto connected = stream.connect(endpoint());
  auto write = stream.write_all(bytes({0x01}));
  std::vector<std::uint8_t> out = bytes({0xff});
  auto read = stream.read_some(&out);

  bool ok = true;
  ok = expect(!connected.ok, "socket hardening failure should fail connect") &&
       ok;
  ok = expect(connected.code == "tls_connect_failed",
              "socket hardening failure should map to tls_connect_failed") &&
       ok;
  ok = expect(connected.message.find(name) != std::string::npos,
              "socket hardening failure should name the failed option") &&
       ok;
  ok = expect(state->open_connected_socket_count == 1,
              "native TCP seam should return one connected socket") &&
       ok;
  ok = expect(state->close_socket_count == 1,
              "failed socket hardening should close the connected socket") &&
       ok;
  ok = expect(state->set_protocol_floor_count == 0,
              "socket hardening failure should happen before TLS setup") &&
       ok;
  ok = expect(!write.ok && write.code == "tls_stream_not_connected",
              "write after socket hardening failure should be not connected") &&
       ok;
  ok = expect(!read.ok && read.code == "tls_stream_not_connected",
              "read after socket hardening failure should be not connected") &&
       ok;
  ok = expect(out == bytes({0xff}),
              "failed not-connected read should not mutate output") &&
       ok;
  return ok;
}

bool socket_timeout_and_no_sigpipe_failures_are_fatal() {
  bool ok = true;
  ok = socket_hardening_failure_fails_before_tls_handshake(SO_RCVTIMEO,
                                                           "SO_RCVTIMEO") &&
       ok;
  ok = socket_hardening_failure_fails_before_tls_handshake(SO_SNDTIMEO,
                                                           "SO_SNDTIMEO") &&
       ok;
#ifdef SO_NOSIGPIPE
  ok = socket_hardening_failure_fails_before_tls_handshake(SO_NOSIGPIPE,
                                                           "SO_NOSIGPIPE") &&
       ok;
#endif
  return ok;
}

bool secure_transport_protocol_floor_failure_is_handshake_error() {
  auto state = std::make_shared<NativeSeamState>();
  state->protocol_floor_result = -9800;
  exv::platform::NativeTlsStream stream(
      exv::platform::make_darwin_native_tls_api(
          native_dependencies(state)));

  auto connected = stream.connect(endpoint());
  auto write = stream.write_all(bytes({0x01}));

  bool ok = true;
  ok = expect(!connected.ok,
              "TLS protocol floor failure should fail connect") &&
       ok;
  ok = expect(connected.code == "tls_handshake_failed",
              "TLS protocol floor failure should be a handshake/config error") &&
       ok;
  ok = expect(connected.message.find("TLS 1.2") != std::string::npos,
              "TLS protocol floor failure should name the TLS 1.2 floor") &&
       ok;
  ok = expect(state->set_protocol_floor_count == 1,
              "TLS setup should set an explicit minimum protocol once") &&
       ok;
  ok = expect(state->last_protocol_floor ==
                  exv::platform::DarwinNativeTlsProtocolVersion::tls12,
              "TLS setup should require TLS 1.2 as the protocol floor") &&
       ok;
  ok = expect(state->close_socket_count == 1,
              "TLS protocol floor failure should close the connected socket") &&
       ok;
  ok = expect(!write.ok && write.code == "tls_stream_not_connected",
              "write after TLS setup failure should be not connected") &&
       ok;
  return ok;
}

bool success_uses_sni_and_writes_plaintext() {
  MockDarwinTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);

  auto connected = stream->connect(endpoint());
  auto written = stream->write_all(bytes({0x48, 0x54, 0x54, 0x50}));
  stream->close();

  bool ok = true;
  ok = expect(connected.ok, "connect should succeed with mocked API") && ok;
  ok = expect(written.ok, "write_all should succeed with mocked API") && ok;
  ok = expect(mock->connect_count == 1, "connect should open one TCP socket") &&
       ok;
  ok = expect(mock->last_endpoint.host == "vpn.example.invalid" &&
                  mock->last_endpoint.port == 4443,
              "TCP connect should use endpoint host and port") &&
       ok;
  ok = expect(mock->handshake_count == 1,
              "connect should perform one TLS handshake") &&
       ok;
  ok = expect(mock->last_sni == "sni.example.invalid",
              "TLS handshake should use endpoint SNI host") &&
       ok;
  ok = expect(mock->write_count == 1,
              "write_all should send one plaintext payload") &&
       ok;
  ok = expect(mock->last_write == bytes({0x48, 0x54, 0x54, 0x50}),
              "write_all should preserve the full plaintext payload") &&
       ok;
  ok = expect(mock->close_context_count == 1,
              "close should release TLS context") &&
       ok;
  ok = expect(mock->close_socket_count == 1, "close should close socket") && ok;
  return ok;
}

bool empty_sni_falls_back_to_endpoint_host() {
  MockDarwinTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  auto target = endpoint();
  target.sni_host.clear();

  auto connected = stream->connect(target);
  stream->close();

  bool ok = true;
  ok = expect(connected.ok, "connect should succeed with host fallback SNI") &&
       ok;
  ok = expect(mock->handshake_count == 1,
              "connect should still perform TLS handshake") &&
       ok;
  ok = expect(mock->last_sni == "vpn.example.invalid",
              "empty endpoint SNI should fall back to endpoint host") &&
       ok;
  return ok;
}

bool empty_host_and_sni_fails_before_network_io() {
  MockDarwinTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  auto target = endpoint();
  target.host.clear();
  target.sni_host.clear();

  auto connected = stream->connect(target);

  bool ok = true;
  ok = expect(!connected.ok, "empty host and SNI should fail connect") && ok;
  ok = expect(connected.code == "tls_endpoint_invalid",
              "empty host and SNI should use stable endpoint validation code") &&
       ok;
  ok = expect(mock->connect_count == 0,
              "endpoint validation should happen before TCP connect") &&
       ok;
  ok = expect(mock->handshake_count == 0,
              "endpoint validation should happen before TLS handshake") &&
       ok;
  return ok;
}

bool connect_failure_maps_to_tls_connect_failed_and_leaves_not_connected() {
  MockDarwinTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  mock->tcp_connect_result.result =
      invalid("tls_connect_failed", "TCP connect timed out");
  mock->tcp_connect_result.socket =
      exv::platform::kInvalidDarwinNativeTlsSocketHandle;

  auto connected = stream->connect(endpoint());
  auto write = stream->write_all(bytes({0x01}));
  std::vector<std::uint8_t> out = bytes({0xff});
  auto read = stream->read_some(&out);

  bool ok = true;
  ok = expect(!connected.ok, "TCP failure should fail connect") && ok;
  ok = expect(connected.code == "tls_connect_failed",
              "TCP failure should map to tls_connect_failed") &&
       ok;
  ok = expect(mock->connect_count == 1,
              "TCP connect should be attempted once") &&
       ok;
  ok = expect(mock->handshake_count == 0,
              "TCP failure should not start TLS handshake") &&
       ok;
  ok = expect(!write.ok && write.code == "tls_stream_not_connected",
              "write after failed connect should fail as not connected") &&
       ok;
  ok = expect(!read.ok && read.code == "tls_stream_not_connected",
              "read after failed connect should fail as not connected") &&
       ok;
  ok = expect(out == bytes({0xff}),
              "failed not-connected read should not mutate output") &&
       ok;
  return ok;
}

bool handshake_failure_maps_to_tls_handshake_failed_and_closes_socket() {
  MockDarwinTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  mock->handshake_result.tls_context =
      exv::platform::kInvalidDarwinNativeTlsContextHandle;
  mock->handshake_result.result =
      invalid("tls_handshake_failed", "TLS handshake failed");

  auto connected = stream->connect(endpoint());

  bool ok = true;
  ok = expect(!connected.ok, "handshake failure should fail connect") && ok;
  ok = expect(connected.code == "tls_handshake_failed",
              "handshake failure should keep stable failure code") &&
       ok;
  ok = expect(mock->close_context_count == 0,
              "handshake failure without context should not close context") &&
       ok;
  ok = expect(mock->close_socket_count == 1,
              "handshake failure should close TCP socket") &&
       ok;
  return ok;
}

bool tls_verification_failure_maps_to_tls_verify_failed_and_cleans_up() {
  MockDarwinTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  mock->handshake_result.tls_context = 202;
  mock->handshake_result.result =
      invalid("tls_verify_failed", "certificate verification failed");

  auto connected = stream->connect(endpoint());

  bool ok = true;
  ok = expect(!connected.ok,
              "certificate verification failure should fail connect") &&
       ok;
  ok = expect(connected.code == "tls_verify_failed",
              "certificate verification failure should map to tls_verify_failed") &&
       ok;
  ok = expect(mock->close_context_count == 1,
              "failed verification should release TLS context") &&
       ok;
  ok = expect(mock->close_socket_count == 1,
              "failed verification should close TCP socket") &&
       ok;
  return ok;
}

bool partial_reads_return_plaintext_chunks() {
  MockDarwinTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  mock->read_results.push_back({{}, bytes({0x48, 0x54}), false});
  mock->read_results.push_back({{}, bytes({0x54, 0x50}), false});

  auto connected = stream->connect(endpoint());
  std::vector<std::uint8_t> out1;
  std::vector<std::uint8_t> out2;
  auto read1 = stream->read_some(&out1);
  auto read2 = stream->read_some(&out2);

  bool ok = true;
  ok = expect(connected.ok, "connect should succeed before partial reads") &&
       ok;
  ok = expect(read1.ok && out1 == bytes({0x48, 0x54}),
              "first read should return first plaintext chunk") &&
       ok;
  ok = expect(read2.ok && out2 == bytes({0x54, 0x50}),
              "second read should return second plaintext chunk") &&
       ok;
  ok = expect(mock->read_count == 2,
              "each partial plaintext read should call the native API once") &&
       ok;
  return ok;
}

bool peer_close_returns_clean_eof() {
  MockDarwinTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  exv::platform::DarwinNativeTlsReadResult peer_closed;
  peer_closed.peer_closed = true;
  mock->read_results.push_back(peer_closed);

  auto connected = stream->connect(endpoint());
  std::vector<std::uint8_t> out = bytes({0xff});
  auto read = stream->read_some(&out);
  stream->close();

  bool ok = true;
  ok = expect(connected.ok, "connect should succeed before TCP EOF") && ok;
  ok = expect(read.ok, "peer close should be a clean read result") && ok;
  ok = expect(out.empty(),
              "peer close should return an empty byte vector") &&
       ok;
  ok = expect(mock->close_context_count == 1,
              "EOF should release TLS context once") &&
       ok;
  ok = expect(mock->close_socket_count == 1,
              "EOF should close socket once") &&
       ok;
  return ok;
}

bool close_is_idempotent() {
  MockDarwinTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);

  auto connected = stream->connect(endpoint());
  stream->close();
  stream->close();
  stream->close();

  auto write = stream->write_all(bytes({0x01}));
  std::vector<std::uint8_t> out;
  auto read = stream->read_some(&out);

  bool ok = true;
  ok = expect(connected.ok, "connect should succeed before close") && ok;
  ok = expect(mock->close_context_count == 1,
              "close should release TLS context once") &&
       ok;
  ok = expect(mock->close_socket_count == 1,
              "close should close socket once") &&
       ok;
  ok = expect(!write.ok && write.code == "tls_stream_closed",
              "write after close should fail as closed") &&
       ok;
  ok = expect(!read.ok && read.code == "tls_stream_closed",
              "read after close should fail as closed") &&
       ok;
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = socket_timeout_and_no_sigpipe_failures_are_fatal() && ok;
  ok = secure_transport_protocol_floor_failure_is_handshake_error() && ok;
  ok = success_uses_sni_and_writes_plaintext() && ok;
  ok = empty_sni_falls_back_to_endpoint_host() && ok;
  ok = empty_host_and_sni_fails_before_network_io() && ok;
  ok = connect_failure_maps_to_tls_connect_failed_and_leaves_not_connected() &&
       ok;
  ok = handshake_failure_maps_to_tls_handshake_failed_and_closes_socket() && ok;
  ok = tls_verification_failure_maps_to_tls_verify_failed_and_cleans_up() && ok;
  ok = partial_reads_return_plaintext_chunks() && ok;
  ok = peer_close_returns_clean_eof() && ok;
  ok = close_is_idempotent() && ok;
  return ok ? 0 : 1;
}
