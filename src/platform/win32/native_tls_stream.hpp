#pragma once

#include "vpn_engine/protocol/tls_stream.hpp"

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

namespace exv {
namespace platform {

using NativeTlsSocketHandle = std::uintptr_t;
using NativeTlsContextHandle = std::uintptr_t;

constexpr NativeTlsSocketHandle kInvalidNativeTlsSocketHandle =
    static_cast<NativeTlsSocketHandle>(~NativeTlsSocketHandle{0});
constexpr NativeTlsContextHandle kInvalidNativeTlsContextHandle =
    NativeTlsContextHandle{0};

enum class NativeTlsReadStatus {
  data,
  need_more_data,
  peer_closed,
  tls_alert,
};

struct NativeTlsTcpConnectResult {
  vpn_engine::ValidationResult result;
  NativeTlsSocketHandle socket = kInvalidNativeTlsSocketHandle;
};

struct NativeTlsHandshakeResult {
  vpn_engine::ValidationResult result;
  NativeTlsContextHandle tls_context = kInvalidNativeTlsContextHandle;
  std::vector<std::uint8_t> encrypted_extra;
};

struct NativeTlsRecvResult {
  vpn_engine::ValidationResult result;
  std::vector<std::uint8_t> bytes;
  bool peer_closed = false;
};

struct NativeTlsDecryptResult {
  vpn_engine::ValidationResult result;
  NativeTlsReadStatus status = NativeTlsReadStatus::data;
  std::vector<std::uint8_t> plaintext;
  std::size_t encrypted_bytes_consumed = 0;
};

class NativeTlsApi {
public:
  virtual ~NativeTlsApi() = default;

  virtual vpn_engine::ValidationResult startup() = 0;
  virtual NativeTlsTcpConnectResult connect_tcp(
      const vpn_engine::protocol::TlsEndpoint &endpoint) = 0;
  virtual NativeTlsHandshakeResult
  handshake(NativeTlsSocketHandle socket, const std::string &sni_host) = 0;
  virtual vpn_engine::ValidationResult
  send_plaintext(NativeTlsContextHandle tls_context,
                 NativeTlsSocketHandle socket,
                 const std::vector<std::uint8_t> &bytes) = 0;
  virtual NativeTlsRecvResult recv_encrypted(NativeTlsSocketHandle socket) = 0;
  virtual NativeTlsDecryptResult
  decrypt(NativeTlsContextHandle tls_context,
          const std::vector<std::uint8_t> &encrypted) = 0;
  virtual void close_tls_context(NativeTlsContextHandle tls_context) = 0;
  virtual void close_socket(NativeTlsSocketHandle socket) = 0;
  virtual void shutdown() = 0;
};

std::unique_ptr<NativeTlsApi> make_windows_native_tls_api();

class NativeTlsStream final : public vpn_engine::protocol::TlsStream {
public:
  NativeTlsStream();
  explicit NativeTlsStream(std::unique_ptr<NativeTlsApi> api);
  ~NativeTlsStream() override;

  NativeTlsStream(const NativeTlsStream &) = delete;
  NativeTlsStream &operator=(const NativeTlsStream &) = delete;

  vpn_engine::ValidationResult
  connect(const vpn_engine::protocol::TlsEndpoint &endpoint) override;
  vpn_engine::ValidationResult
  write_all(const std::vector<std::uint8_t> &bytes) override;
  vpn_engine::ValidationResult
  read_some(std::vector<std::uint8_t> *bytes) override;
  void close() override;

private:
  vpn_engine::ValidationResult ensure_open() const;
  vpn_engine::ValidationResult fail_and_close(vpn_engine::ValidationResult result);
  vpn_engine::ValidationResult
  fail_connect_and_reset(vpn_engine::ValidationResult result);

  std::unique_ptr<NativeTlsApi> api_;
  NativeTlsSocketHandle socket_ = kInvalidNativeTlsSocketHandle;
  NativeTlsContextHandle tls_context_ = kInvalidNativeTlsContextHandle;
  std::vector<std::uint8_t> encrypted_buffer_;
  bool api_started_ = false;
  bool connected_ = false;
  std::atomic<bool> closed_{false};

  // Serializes inbound read_some() decrypt/teardown against close() so the
  // Schannel context and socket are never destroyed while the reader thread is
  // mid-DecryptMessage. The blocking recv() in read_some() runs OUTSIDE this
  // lock; close() unblocks it by closing the socket. write_all() runs on the
  // same thread as close() in the full-duplex session, and Schannel permits a
  // concurrent encrypt (writer) with a decrypt (reader) on one context, so the
  // writer does not take this lock across its blocking send().
  std::mutex io_mutex_;
};

} // namespace platform
} // namespace exv
