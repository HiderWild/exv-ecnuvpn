#pragma once

#include "vpn_engine/protocol/tls_stream.hpp"

#include <cstdint>
#include <cstddef>
#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace platform {

using DarwinNativeTlsSocketHandle = int;
using DarwinNativeTlsContextHandle = std::uintptr_t;

constexpr DarwinNativeTlsSocketHandle kInvalidDarwinNativeTlsSocketHandle = -1;
constexpr DarwinNativeTlsContextHandle kInvalidDarwinNativeTlsContextHandle =
    DarwinNativeTlsContextHandle{0};

struct DarwinNativeTlsTcpConnectResult {
  vpn_engine::ValidationResult result;
  DarwinNativeTlsSocketHandle socket = kInvalidDarwinNativeTlsSocketHandle;
};

struct DarwinNativeTlsHandshakeResult {
  vpn_engine::ValidationResult result;
  DarwinNativeTlsContextHandle tls_context =
      kInvalidDarwinNativeTlsContextHandle;
};

struct DarwinNativeTlsReadResult {
  vpn_engine::ValidationResult result;
  std::vector<std::uint8_t> bytes;
  bool peer_closed = false;
};

using DarwinNativeTlsSecureTransportContextHandle = std::uintptr_t;

enum class DarwinNativeTlsProtocolVersion {
  tls12,
};

struct DarwinNativeTlsTcpApi {
  std::function<DarwinNativeTlsTcpConnectResult(
      const vpn_engine::protocol::TlsEndpoint &, int)>
      open_connected_socket;
  std::function<void(DarwinNativeTlsSocketHandle)> close_socket;
};

struct DarwinNativeTlsSocketOptionsApi {
  std::function<int(DarwinNativeTlsSocketHandle, int, int, const void *,
                    std::size_t)>
      set_socket_option;
  std::function<int()> last_error;
};

struct DarwinNativeTlsSecureTransportApi {
  std::function<int(DarwinNativeTlsSecureTransportContextHandle,
                    DarwinNativeTlsProtocolVersion)>
      set_protocol_version_min;
};

struct DarwinNativeTlsDependencies {
  DarwinNativeTlsTcpApi tcp;
  DarwinNativeTlsSocketOptionsApi socket_options;
  DarwinNativeTlsSecureTransportApi secure_transport;
};

class DarwinNativeTlsApi {
public:
  virtual ~DarwinNativeTlsApi() = default;

  virtual DarwinNativeTlsTcpConnectResult
  connect_tcp(const vpn_engine::protocol::TlsEndpoint &endpoint) = 0;
  virtual DarwinNativeTlsHandshakeResult
  handshake(DarwinNativeTlsSocketHandle socket,
            const std::string &sni_host) = 0;
  virtual vpn_engine::ValidationResult
  write_plaintext(DarwinNativeTlsContextHandle tls_context,
                  DarwinNativeTlsSocketHandle socket,
                  const std::vector<std::uint8_t> &bytes) = 0;
  virtual DarwinNativeTlsReadResult
  read_plaintext(DarwinNativeTlsContextHandle tls_context,
                 DarwinNativeTlsSocketHandle socket) = 0;
  virtual void close_tls_context(DarwinNativeTlsContextHandle tls_context) = 0;
  virtual void close_socket(DarwinNativeTlsSocketHandle socket) = 0;
};

DarwinNativeTlsDependencies default_darwin_native_tls_dependencies();
std::unique_ptr<DarwinNativeTlsApi> make_darwin_native_tls_api();
std::unique_ptr<DarwinNativeTlsApi>
make_darwin_native_tls_api(DarwinNativeTlsDependencies dependencies);

class NativeTlsStream final : public vpn_engine::protocol::TlsStream {
public:
  NativeTlsStream();
  explicit NativeTlsStream(std::unique_ptr<DarwinNativeTlsApi> api);
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

  std::unique_ptr<DarwinNativeTlsApi> api_;
  DarwinNativeTlsSocketHandle socket_ = kInvalidDarwinNativeTlsSocketHandle;
  DarwinNativeTlsContextHandle tls_context_ =
      kInvalidDarwinNativeTlsContextHandle;
  bool connected_ = false;
  bool closed_ = false;
};

} // namespace platform
} // namespace ecnuvpn
