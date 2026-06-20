#include "platform/win32/native_tls_stream.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef SECURITY_WIN32
#define SECURITY_WIN32
#endif

#include <winsock2.h>
#include <ws2tcpip.h>

#include <windows.h>
#include <schannel.h>
#include <security.h>

#include <cstdint>
#include <deque>
#include <iostream>
#include <memory>
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

struct NativeWindowsTlsMock {
  int wsa_startup_count = 0;
  int wsa_cleanup_count = 0;
  int getaddrinfo_count = 0;
  int freeaddrinfo_count = 0;
  int socket_count = 0;
  int ioctlsocket_count = 0;
  int connect_count = 0;
  int setsockopt_count = 0;
  int send_count = 0;
  int closesocket_count = 0;
  int acquire_credentials_count = 0;
  int initialize_context_count = 0;
  int delete_context_count = 0;
  int free_credentials_count = 0;
  int free_context_buffer_count = 0;
  SOCKET fake_socket = static_cast<SOCKET>(101);
  int send_result = SOCKET_ERROR;
  int last_wsa_error = WSAECONNRESET;
  SECURITY_STATUS initialize_status = SEC_I_CONTINUE_NEEDED;
  char output_token[3] = {0x16, 0x03, 0x01};
  std::vector<std::uint8_t> sent_bytes;
  sockaddr_in address_storage{};
  addrinfo address{};
};

NativeWindowsTlsMock g_windows_tls_mock;

void reset_windows_tls_mock() {
  g_windows_tls_mock = NativeWindowsTlsMock{};
  g_windows_tls_mock.address_storage.sin_family = AF_INET;
  g_windows_tls_mock.address.ai_family = AF_INET;
  g_windows_tls_mock.address.ai_socktype = SOCK_STREAM;
  g_windows_tls_mock.address.ai_protocol = IPPROTO_TCP;
  g_windows_tls_mock.address.ai_addr =
      reinterpret_cast<sockaddr *>(&g_windows_tls_mock.address_storage);
  g_windows_tls_mock.address.ai_addrlen =
      static_cast<int>(sizeof(g_windows_tls_mock.address_storage));
}

int WSAAPI fake_wsa_startup(WORD, LPWSADATA data) {
  ++g_windows_tls_mock.wsa_startup_count;
  if (data)
    *data = WSADATA{};
  return 0;
}

int WSAAPI fake_wsa_cleanup() {
  ++g_windows_tls_mock.wsa_cleanup_count;
  return 0;
}

int WSAAPI fake_wsa_get_last_error() {
  return g_windows_tls_mock.last_wsa_error;
}

int WSAAPI fake_getaddrinfo(PCSTR, PCSTR, const ADDRINFOA *,
                            PADDRINFOA *result) {
  ++g_windows_tls_mock.getaddrinfo_count;
  if (result)
    *result = &g_windows_tls_mock.address;
  return 0;
}

void WSAAPI fake_freeaddrinfo(PADDRINFOA) {
  ++g_windows_tls_mock.freeaddrinfo_count;
}

SOCKET WSAAPI fake_socket(int, int, int) {
  ++g_windows_tls_mock.socket_count;
  return g_windows_tls_mock.fake_socket;
}

int WSAAPI fake_ioctlsocket(SOCKET, long, u_long *) {
  ++g_windows_tls_mock.ioctlsocket_count;
  return 0;
}

int WSAAPI fake_connect(SOCKET, const sockaddr *, int) {
  ++g_windows_tls_mock.connect_count;
  return 0;
}

int WSAAPI fake_select(int, fd_set *, fd_set *, fd_set *, const timeval *) {
  return 1;
}

int WSAAPI fake_getsockopt(SOCKET, int, int, char *value, int *value_size) {
  if (value && value_size && *value_size >= static_cast<int>(sizeof(int))) {
    *reinterpret_cast<int *>(value) = 0;
    *value_size = sizeof(int);
  }
  return 0;
}

int WSAAPI fake_setsockopt(SOCKET, int, int, const char *, int) {
  ++g_windows_tls_mock.setsockopt_count;
  return 0;
}

int WSAAPI fake_send(SOCKET, const char *data, int size, int) {
  ++g_windows_tls_mock.send_count;
  if (data && size > 0) {
    g_windows_tls_mock.sent_bytes.assign(
        reinterpret_cast<const std::uint8_t *>(data),
        reinterpret_cast<const std::uint8_t *>(data) + size);
  }
  return g_windows_tls_mock.send_result;
}

int WSAAPI fake_recv(SOCKET, char *, int, int) {
  return SOCKET_ERROR;
}

int WSAAPI fake_closesocket(SOCKET) {
  ++g_windows_tls_mock.closesocket_count;
  return 0;
}

extern "C" SECURITY_STATUS SEC_ENTRY AcquireCredentialsHandleA(
    SEC_CHAR *, SEC_CHAR *, unsigned long, void *, void *, SEC_GET_KEY_FN,
    void *, PCredHandle credentials, PTimeStamp) {
  ++g_windows_tls_mock.acquire_credentials_count;
  if (credentials) {
    credentials->dwLower = 0x1111;
    credentials->dwUpper = 0x2222;
  }
  return SEC_E_OK;
}

extern "C" SECURITY_STATUS SEC_ENTRY InitializeSecurityContextA(
    PCredHandle, PCtxtHandle, SEC_CHAR *, unsigned long, unsigned long,
    unsigned long, PSecBufferDesc, unsigned long, PCtxtHandle new_context,
    PSecBufferDesc output, unsigned long *, PTimeStamp) {
  ++g_windows_tls_mock.initialize_context_count;
  if (new_context) {
    new_context->dwLower = 0x3333;
    new_context->dwUpper = 0x4444;
  }
  if (output && output->cBuffers > 0 && output->pBuffers) {
    output->pBuffers[0].BufferType = SECBUFFER_TOKEN;
    output->pBuffers[0].pvBuffer = g_windows_tls_mock.output_token;
    output->pBuffers[0].cbBuffer =
        static_cast<unsigned long>(sizeof(g_windows_tls_mock.output_token));
  }
  return g_windows_tls_mock.initialize_status;
}

extern "C" SECURITY_STATUS SEC_ENTRY fake_delete_security_context(
    PCtxtHandle) {
  ++g_windows_tls_mock.delete_context_count;
  return SEC_E_OK;
}

extern "C" SECURITY_STATUS SEC_ENTRY fake_free_credentials_handle(
    PCredHandle) {
  ++g_windows_tls_mock.free_credentials_count;
  return SEC_E_OK;
}

extern "C" SECURITY_STATUS SEC_ENTRY FreeContextBuffer(PVOID) {
  ++g_windows_tls_mock.free_context_buffer_count;
  return SEC_E_OK;
}

} // namespace

extern "C" {
decltype(&WSAStartup) __imp_WSAStartup = fake_wsa_startup;
decltype(&WSACleanup) __imp_WSACleanup = fake_wsa_cleanup;
decltype(&WSAGetLastError) __imp_WSAGetLastError = fake_wsa_get_last_error;
decltype(&getaddrinfo) __imp_getaddrinfo = fake_getaddrinfo;
decltype(&freeaddrinfo) __imp_freeaddrinfo = fake_freeaddrinfo;
decltype(&socket) __imp_socket = fake_socket;
decltype(&ioctlsocket) __imp_ioctlsocket = fake_ioctlsocket;
decltype(&connect) __imp_connect = fake_connect;
decltype(&select) __imp_select = fake_select;
decltype(&getsockopt) __imp_getsockopt = fake_getsockopt;
decltype(&setsockopt) __imp_setsockopt = fake_setsockopt;
decltype(&send) __imp_send = fake_send;
decltype(&recv) __imp_recv = fake_recv;
decltype(&closesocket) __imp_closesocket = fake_closesocket;
decltype(&DeleteSecurityContext) __imp_DeleteSecurityContext =
    fake_delete_security_context;
decltype(&FreeCredentialsHandle) __imp_FreeCredentialsHandle =
    fake_free_credentials_handle;
}

namespace {

class MockNativeTlsApi final : public exv::platform::NativeTlsApi {
public:
  exv::vpn_engine::ValidationResult startup_result;
  exv::platform::NativeTlsTcpConnectResult tcp_connect_result;
  exv::platform::NativeTlsHandshakeResult handshake_result;
  exv::vpn_engine::ValidationResult send_result;
  std::deque<exv::platform::NativeTlsRecvResult> recv_results;
  std::deque<exv::platform::NativeTlsDecryptResult> decrypt_results;

  int startup_count = 0;
  int connect_count = 0;
  int handshake_count = 0;
  int send_count = 0;
  int recv_count = 0;
  int decrypt_count = 0;
  int close_context_count = 0;
  int close_socket_count = 0;
  int shutdown_count = 0;

  exv::vpn_engine::protocol::TlsEndpoint last_endpoint;
  std::string last_sni;
  std::vector<std::uint8_t> last_write;
  std::vector<std::vector<std::uint8_t>> decrypt_inputs;

  MockNativeTlsApi() {
    tcp_connect_result.socket = 101;
    handshake_result.tls_context = 202;
  }

  exv::vpn_engine::ValidationResult startup() override {
    ++startup_count;
    return startup_result;
  }

  exv::platform::NativeTlsTcpConnectResult connect_tcp(
      const exv::vpn_engine::protocol::TlsEndpoint &endpoint) override {
    ++connect_count;
    last_endpoint = endpoint;
    return tcp_connect_result;
  }

  exv::platform::NativeTlsHandshakeResult
  handshake(exv::platform::NativeTlsSocketHandle,
            const std::string &sni_host) override {
    ++handshake_count;
    last_sni = sni_host;
    return handshake_result;
  }

  exv::vpn_engine::ValidationResult send_plaintext(
      exv::platform::NativeTlsContextHandle,
      exv::platform::NativeTlsSocketHandle,
      const std::vector<std::uint8_t> &bytes) override {
    ++send_count;
    last_write = bytes;
    return send_result;
  }

  exv::platform::NativeTlsRecvResult recv_encrypted(
      exv::platform::NativeTlsSocketHandle) override {
    ++recv_count;
    if (recv_results.empty()) {
      exv::platform::NativeTlsRecvResult result;
      result.result = invalid("transport_closed", "no encrypted bytes queued");
      result.peer_closed = true;
      return result;
    }

    exv::platform::NativeTlsRecvResult result = recv_results.front();
    recv_results.pop_front();
    return result;
  }

  exv::platform::NativeTlsDecryptResult decrypt(
      exv::platform::NativeTlsContextHandle,
      const std::vector<std::uint8_t> &encrypted) override {
    ++decrypt_count;
    decrypt_inputs.push_back(encrypted);
    if (decrypt_results.empty()) {
      exv::platform::NativeTlsDecryptResult result;
      result.result = invalid("tls_decrypt_failed", "no decrypt result queued");
      return result;
    }

    exv::platform::NativeTlsDecryptResult result = decrypt_results.front();
    decrypt_results.pop_front();
    return result;
  }

  void close_tls_context(
      exv::platform::NativeTlsContextHandle) override {
    ++close_context_count;
  }

  void close_socket(exv::platform::NativeTlsSocketHandle) override {
    ++close_socket_count;
  }

  void shutdown() override { ++shutdown_count; }
};

std::unique_ptr<exv::platform::NativeTlsStream>
make_stream(MockNativeTlsApi **mock) {
  auto api = std::make_unique<MockNativeTlsApi>();
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

bool success_uses_sni_and_writes_plaintext() {
  MockNativeTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);

  auto connected = stream->connect(endpoint());
  auto written = stream->write_all(bytes({0x48, 0x54, 0x54, 0x50}));
  stream->close();

  bool ok = true;
  ok = expect(connected.ok, "connect should succeed with mocked API") && ok;
  ok = expect(written.ok, "write_all should succeed with mocked API") && ok;
  ok = expect(mock->startup_count == 1, "connect should start Winsock once") &&
       ok;
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
  ok = expect(mock->send_count == 1,
              "write_all should send one plaintext payload") &&
       ok;
  ok = expect(mock->last_write == bytes({0x48, 0x54, 0x54, 0x50}),
              "write_all should preserve the full plaintext payload") &&
       ok;
  ok = expect(mock->close_context_count == 1,
              "close should release TLS context") &&
       ok;
  ok = expect(mock->close_socket_count == 1, "close should close socket") && ok;
  ok = expect(mock->shutdown_count == 1, "close should shutdown Winsock") && ok;
  return ok;
}

bool empty_sni_falls_back_to_endpoint_host() {
  MockNativeTlsApi *mock = nullptr;
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
  MockNativeTlsApi *mock = nullptr;
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
  ok = expect(mock->startup_count == 0,
              "endpoint validation should happen before Winsock startup") &&
       ok;
  ok = expect(mock->connect_count == 0,
              "endpoint validation should happen before TCP connect") &&
       ok;
  ok = expect(mock->handshake_count == 0,
              "endpoint validation should happen before TLS handshake") &&
       ok;
  return ok;
}

bool dns_failure_maps_to_tls_connect_failed() {
  MockNativeTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  mock->tcp_connect_result.result =
      invalid("tls_connect_failed", "DNS resolution failed");
  mock->tcp_connect_result.socket =
      exv::platform::kInvalidNativeTlsSocketHandle;

  auto connected = stream->connect(endpoint());

  bool ok = true;
  ok = expect(!connected.ok, "DNS failure should fail connect") && ok;
  ok = expect(connected.code == "tls_connect_failed",
              "DNS failure should map to tls_connect_failed") &&
       ok;
  ok = expect(mock->handshake_count == 0,
              "DNS failure should not start TLS handshake") &&
       ok;
  ok = expect(mock->close_context_count == 0,
              "DNS failure should not close a missing TLS context") &&
       ok;
  ok = expect(mock->close_socket_count == 0,
              "DNS failure should not close a missing socket") &&
       ok;
  ok = expect(mock->shutdown_count == 1,
              "DNS failure should cleanup Winsock startup") &&
       ok;
  return ok;
}

bool tcp_failure_maps_to_tls_connect_failed() {
  MockNativeTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  mock->tcp_connect_result.result =
      invalid("tls_connect_failed", "TCP connect timed out");
  mock->tcp_connect_result.socket =
      exv::platform::kInvalidNativeTlsSocketHandle;

  auto connected = stream->connect(endpoint());

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
  ok = expect(mock->shutdown_count == 1,
              "TCP failure should cleanup Winsock startup") &&
       ok;
  return ok;
}

bool failed_connect_leaves_stream_not_connected_for_io() {
  MockNativeTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  mock->handshake_result.result =
      invalid("tls_handshake_failed", "TLS handshake failed");

  auto connected = stream->connect(endpoint());
  auto write = stream->write_all(bytes({0x01}));
  std::vector<std::uint8_t> out = bytes({0xff});
  auto read = stream->read_some(&out);

  bool ok = true;
  ok = expect(!connected.ok, "handshake failure should fail connect") && ok;
  ok = expect(connected.code == "tls_handshake_failed",
              "handshake failure should keep original connect failure code") &&
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

bool schannel_send_failure_deletes_created_context_once() {
  reset_windows_tls_mock();
  exv::platform::NativeTlsStream stream(
      exv::platform::make_windows_native_tls_api());

  auto connected = stream.connect(endpoint());
  auto write = stream.write_all(bytes({0x01}));
  stream.close();

  bool ok = true;
  ok = expect(!connected.ok,
              "token send failure should fail the native TLS connect") &&
       ok;
  ok = expect(connected.code == "tls_write_failed",
              "token send failure should keep the socket write error code") &&
       ok;
  ok = expect(!write.ok && write.code == "tls_stream_not_connected",
              "write after native handshake send failure should be not connected") &&
       ok;
  ok = expect(g_windows_tls_mock.acquire_credentials_count == 1,
              "native handshake should acquire credentials once") &&
       ok;
  ok = expect(g_windows_tls_mock.initialize_context_count == 1,
              "native handshake should create one Schannel context") &&
       ok;
  ok = expect(g_windows_tls_mock.send_count == 1,
              "native handshake should try to send one token") &&
       ok;
  ok = expect(g_windows_tls_mock.sent_bytes == bytes({0x16, 0x03, 0x01}),
              "native handshake should send the Schannel output token") &&
       ok;
  ok = expect(g_windows_tls_mock.free_context_buffer_count == 1,
              "failed token send should release the output token buffer once") &&
       ok;
  ok = expect(g_windows_tls_mock.delete_context_count == 1,
              "failed token send should delete the created Schannel context once") &&
       ok;
  ok = expect(g_windows_tls_mock.free_credentials_count == 1,
              "failed token send should free credentials once") &&
       ok;
  ok = expect(g_windows_tls_mock.closesocket_count == 1,
              "failed native connect should close the TCP socket once") &&
       ok;
  ok = expect(g_windows_tls_mock.wsa_cleanup_count == 1,
              "failed native connect should cleanup Winsock once") &&
       ok;
  return ok;
}

bool tls_alert_closes_transport() {
  MockNativeTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  mock->recv_results.push_back({{}, bytes({0x15, 0x03, 0x03}), false});
  exv::platform::NativeTlsDecryptResult alert;
  alert.status = exv::platform::NativeTlsReadStatus::tls_alert;
  alert.result = invalid("tls_alert", "TLS alert received");
  mock->decrypt_results.push_back(alert);

  auto connected = stream->connect(endpoint());
  std::vector<std::uint8_t> out;
  auto read = stream->read_some(&out);
  stream->close();

  bool ok = true;
  ok = expect(connected.ok, "connect should succeed before TLS alert") && ok;
  ok = expect(!read.ok, "TLS alert should fail read") && ok;
  ok = expect(read.code == "tls_alert",
              "TLS alert should map to tls_alert") &&
       ok;
  ok = expect(mock->close_context_count == 1,
              "TLS alert should close TLS context") &&
       ok;
  ok = expect(mock->close_socket_count == 1,
              "TLS alert should close socket") &&
       ok;
  ok = expect(mock->shutdown_count == 1,
              "TLS alert should cleanup Winsock startup") &&
       ok;
  return ok;
}

bool tcp_peer_close_returns_clean_eof() {
  MockNativeTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  exv::platform::NativeTlsRecvResult peer_closed;
  peer_closed.peer_closed = true;
  mock->recv_results.push_back(peer_closed);

  auto connected = stream->connect(endpoint());
  std::vector<std::uint8_t> out = bytes({0xff});
  auto read = stream->read_some(&out);
  stream->close();

  bool ok = true;
  ok = expect(connected.ok, "connect should succeed before TCP EOF") && ok;
  ok = expect(read.ok, "TCP peer close should be a clean read result") && ok;
  ok = expect(out.empty(),
              "TCP peer close should return an empty byte vector") &&
       ok;
  ok = expect(mock->close_context_count == 1,
              "TCP EOF should release TLS context once") &&
       ok;
  ok = expect(mock->close_socket_count == 1,
              "TCP EOF should close socket once") &&
       ok;
  ok = expect(mock->shutdown_count == 1,
              "TCP EOF should cleanup Winsock startup once") &&
       ok;
  return ok;
}

bool schannel_peer_closed_returns_clean_eof() {
  MockNativeTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  mock->recv_results.push_back({{}, bytes({0x17, 0x03, 0x03}), false});

  exv::platform::NativeTlsDecryptResult peer_closed;
  peer_closed.status = exv::platform::NativeTlsReadStatus::peer_closed;
  peer_closed.encrypted_bytes_consumed = 3;
  mock->decrypt_results.push_back(peer_closed);

  auto connected = stream->connect(endpoint());
  std::vector<std::uint8_t> out = bytes({0xff});
  auto read = stream->read_some(&out);
  stream->close();

  bool ok = true;
  ok = expect(connected.ok,
              "connect should succeed before Schannel close-notify") &&
       ok;
  ok = expect(read.ok,
              "Schannel peer_closed should be a clean read result") &&
       ok;
  ok = expect(out.empty(),
              "Schannel peer_closed should return an empty byte vector") &&
       ok;
  ok = expect(mock->close_context_count == 1,
              "Schannel EOF should release TLS context once") &&
       ok;
  ok = expect(mock->close_socket_count == 1,
              "Schannel EOF should close socket once") &&
       ok;
  ok = expect(mock->shutdown_count == 1,
              "Schannel EOF should cleanup Winsock startup once") &&
       ok;
  return ok;
}

bool verification_failure_maps_to_tls_verify_failed_and_cleans_up() {
  MockNativeTlsApi *mock = nullptr;
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
  ok = expect(mock->shutdown_count == 1,
              "failed verification should cleanup Winsock startup") &&
       ok;
  return ok;
}

bool preserves_encrypted_extra_between_partial_reads() {
  MockNativeTlsApi *mock = nullptr;
  auto stream = make_stream(&mock);
  mock->recv_results.push_back({{}, bytes({0x01, 0x02, 0x03, 0x04}), false});

  exv::platform::NativeTlsDecryptResult first;
  first.status = exv::platform::NativeTlsReadStatus::data;
  first.plaintext = bytes({0x48, 0x54, 0x54, 0x50});
  first.encrypted_bytes_consumed = 2;
  mock->decrypt_results.push_back(first);

  exv::platform::NativeTlsDecryptResult second;
  second.status = exv::platform::NativeTlsReadStatus::data;
  second.plaintext = bytes({0x2f, 0x31, 0x2e, 0x31});
  second.encrypted_bytes_consumed = 2;
  mock->decrypt_results.push_back(second);

  auto connected = stream->connect(endpoint());
  std::vector<std::uint8_t> out1;
  std::vector<std::uint8_t> out2;
  auto read1 = stream->read_some(&out1);
  auto read2 = stream->read_some(&out2);

  bool ok = true;
  ok = expect(connected.ok, "connect should succeed before partial read test") &&
       ok;
  ok = expect(read1.ok && out1 == bytes({0x48, 0x54, 0x54, 0x50}),
              "first read should return first decrypted record") &&
       ok;
  ok = expect(read2.ok && out2 == bytes({0x2f, 0x31, 0x2e, 0x31}),
              "second read should return data from buffered encrypted extra") &&
       ok;
  ok = expect(mock->recv_count == 1,
              "buffered encrypted extra should avoid a second socket read") &&
       ok;
  ok = expect(mock->decrypt_inputs.size() == 2 &&
                  mock->decrypt_inputs[0] == bytes({0x01, 0x02, 0x03, 0x04}) &&
                  mock->decrypt_inputs[1] == bytes({0x03, 0x04}),
              "read_some should retain unconsumed encrypted bytes") &&
       ok;
  return ok;
}

bool close_is_idempotent() {
  MockNativeTlsApi *mock = nullptr;
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
  ok = expect(mock->shutdown_count == 1,
              "close should shutdown Winsock once") &&
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
  ok = success_uses_sni_and_writes_plaintext() && ok;
  ok = empty_sni_falls_back_to_endpoint_host() && ok;
  ok = empty_host_and_sni_fails_before_network_io() && ok;
  ok = dns_failure_maps_to_tls_connect_failed() && ok;
  ok = tcp_failure_maps_to_tls_connect_failed() && ok;
  ok = failed_connect_leaves_stream_not_connected_for_io() && ok;
  ok = schannel_send_failure_deletes_created_context_once() && ok;
  ok = tls_alert_closes_transport() && ok;
  ok = tcp_peer_close_returns_clean_eof() && ok;
  ok = schannel_peer_closed_returns_clean_eof() && ok;
  ok = verification_failure_maps_to_tls_verify_failed_and_cleans_up() && ok;
  ok = preserves_encrypted_extra_between_partial_reads() && ok;
  ok = close_is_idempotent() && ok;
  return ok ? 0 : 1;
}
