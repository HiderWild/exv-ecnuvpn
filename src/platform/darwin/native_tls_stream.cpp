#include "platform/darwin/native_tls_stream.hpp"

#include <Security/SecureTransport.h>
#include <Security/Security.h>
#include <CoreFoundation/CoreFoundation.h>

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <unistd.h>

#include <algorithm>
#include <cstddef>
#include <cstring>
#include <limits>
#include <sstream>
#include <string>
#include <utility>

namespace exv {
namespace platform {

namespace {

constexpr int kConnectTimeoutMs = 15000;
constexpr std::size_t kPlaintextReadSize = 16 * 1024;
constexpr int kMaxHandshakeSteps = 128;
// Begin inlined from platform/darwin/native_tls_stream_errors include-unit
vpn_engine::ValidationResult invalid(std::string code, std::string message) {
  vpn_engine::ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

vpn_engine::ValidationResult connect_failed(std::string message) {
  return invalid("tls_connect_failed", std::move(message));
}

vpn_engine::ValidationResult handshake_failed(std::string message) {
  return invalid("tls_handshake_failed", std::move(message));
}

vpn_engine::ValidationResult verify_failed(std::string message) {
  return invalid("tls_verify_failed", std::move(message));
}

std::string osstatus_message(const char *operation, OSStatus status) {
  std::ostringstream out;
  out << operation << " failed with OSStatus " << status;
  return out.str();
}

std::string errno_message(const char *operation, int error) {
  std::ostringstream out;
  out << operation << " failed with errno " << error << " ("
      << std::strerror(error) << ")";
  return out.str();
}

std::string socket_option_message(const char *option, int error) {
  std::ostringstream out;
  out << "setsockopt(" << option << ") failed with errno " << error << " ("
      << std::strerror(error) << ")";
  return out.str();
}

bool valid_socket_handle(DarwinNativeTlsSocketHandle socket) {
  return socket != kInvalidDarwinNativeTlsSocketHandle;
}

bool valid_context_handle(DarwinNativeTlsContextHandle context) {
  return context != kInvalidDarwinNativeTlsContextHandle;
}
// End inlined from platform/darwin/native_tls_stream_errors include-unit
// Begin inlined from platform/darwin/native_tls_stream_cf_socket include-unit
template <typename T> class ScopedCfRef {
public:
  ScopedCfRef() = default;
  explicit ScopedCfRef(T ref) : ref_(ref) {}
  ~ScopedCfRef() { reset(); }

  ScopedCfRef(const ScopedCfRef &) = delete;
  ScopedCfRef &operator=(const ScopedCfRef &) = delete;

  T get() const { return ref_; }
  T *out() { return &ref_; }

  T release() {
    T released = ref_;
    ref_ = nullptr;
    return released;
  }

  void reset(T ref = nullptr) {
    if (ref_)
      CFRelease(ref_);
    ref_ = ref;
  }

private:
  T ref_ = nullptr;
};

struct AddrInfoDeleter {
  void operator()(addrinfo *info) const {
    if (info)
      freeaddrinfo(info);
  }
};

bool set_socket_nonblocking(int socket, bool nonblocking) {
  const int flags = fcntl(socket, F_GETFL, 0);
  if (flags < 0)
    return false;

  const int updated =
      nonblocking ? (flags | O_NONBLOCK) : (flags & ~O_NONBLOCK);
  return fcntl(socket, F_SETFL, updated) == 0;
}

bool wait_for_connect(int socket, int timeout_ms) {
  fd_set write_set;
  FD_ZERO(&write_set);
  FD_SET(socket, &write_set);

  fd_set error_set;
  FD_ZERO(&error_set);
  FD_SET(socket, &error_set);

  timeval timeout;
  timeout.tv_sec = timeout_ms / 1000;
  timeout.tv_usec = (timeout_ms % 1000) * 1000;

  const int selected =
      select(socket + 1, nullptr, &write_set, &error_set, &timeout);
  if (selected <= 0)
    return false;

  int socket_error = 0;
  socklen_t socket_error_size = sizeof(socket_error);
  if (getsockopt(socket, SOL_SOCKET, SO_ERROR, &socket_error,
                 &socket_error_size) != 0) {
    return false;
  }

  if (socket_error != 0) {
    errno = socket_error;
    return false;
  }

  return true;
}

bool connect_socket_with_timeout(int socket, const sockaddr *address,
                                 socklen_t address_length, int timeout_ms) {
  if (!set_socket_nonblocking(socket, true))
    return false;

  const int connected = ::connect(socket, address, address_length);
  if (connected == 0) {
    return set_socket_nonblocking(socket, false);
  }

  if (errno != EINPROGRESS && errno != EWOULDBLOCK && errno != EALREADY)
    return false;

  if (!wait_for_connect(socket, timeout_ms))
    return false;

  return set_socket_nonblocking(socket, false);
}

int socket_option_error(const DarwinNativeTlsSocketOptionsApi &api) {
  if (!api.last_error)
    return errno == 0 ? EIO : errno;
  const int error = api.last_error();
  return error == 0 ? EIO : error;
}

bool has_required_tcp_api(const DarwinNativeTlsTcpApi &api) {
  return static_cast<bool>(api.open_connected_socket) &&
         static_cast<bool>(api.close_socket);
}

bool has_required_socket_options_api(
    const DarwinNativeTlsSocketOptionsApi &api) {
  return static_cast<bool>(api.set_socket_option);
}

bool has_required_secure_transport_api(
    const DarwinNativeTlsSecureTransportApi &api) {
  return static_cast<bool>(api.set_protocol_version_min);
}

vpn_engine::ValidationResult
set_socket_option(const DarwinNativeTlsSocketOptionsApi &api,
                  DarwinNativeTlsSocketHandle socket, int level, int option,
                  const char *option_name, const void *value,
                  std::size_t value_size) {
  if (api.set_socket_option(socket, level, option, value, value_size) == 0)
    return {};

  return connect_failed(
      socket_option_message(option_name, socket_option_error(api)));
}

vpn_engine::ValidationResult configure_socket(
    DarwinNativeTlsSocketHandle socket,
    const DarwinNativeTlsSocketOptionsApi &api) {
  if (!has_required_socket_options_api(api)) {
    return connect_failed(
        "native TLS socket options API table is incomplete");
  }

  timeval timeout;
  timeout.tv_sec = kConnectTimeoutMs / 1000;
  timeout.tv_usec = (kConnectTimeoutMs % 1000) * 1000;

  vpn_engine::ValidationResult receive_timeout =
      set_socket_option(api, socket, SOL_SOCKET, SO_RCVTIMEO, "SO_RCVTIMEO",
                        &timeout, sizeof(timeout));
  if (!receive_timeout.ok)
    return receive_timeout;

  vpn_engine::ValidationResult send_timeout =
      set_socket_option(api, socket, SOL_SOCKET, SO_SNDTIMEO, "SO_SNDTIMEO",
                        &timeout, sizeof(timeout));
  if (!send_timeout.ok)
    return send_timeout;

#ifdef SO_NOSIGPIPE
  int no_sigpipe = 1;
  vpn_engine::ValidationResult no_sigpipe_result =
      set_socket_option(api, socket, SOL_SOCKET, SO_NOSIGPIPE, "SO_NOSIGPIPE",
                        &no_sigpipe, sizeof(no_sigpipe));
  if (!no_sigpipe_result.ok)
    return no_sigpipe_result;
#endif

  return {};
}
// End inlined from platform/darwin/native_tls_stream_cf_socket include-unit
// Begin inlined from platform/darwin/native_tls_stream_context include-unit
struct DarwinTlsIo {
  int socket = -1;
};

struct DarwinTlsContext {
  SSLContextRef context = nullptr;
  DarwinTlsIo io;
};

DarwinTlsContext *
context_from_handle(DarwinNativeTlsContextHandle handle) {
  return reinterpret_cast<DarwinTlsContext *>(handle);
}

DarwinNativeTlsContextHandle handle_from_context(DarwinTlsContext *context) {
  if (!context)
    return kInvalidDarwinNativeTlsContextHandle;
  return reinterpret_cast<DarwinNativeTlsContextHandle>(context);
}

DarwinNativeTlsSecureTransportContextHandle
handle_from_secure_transport_context(SSLContextRef context) {
  if (!context)
    return DarwinNativeTlsSecureTransportContextHandle{0};
  return reinterpret_cast<DarwinNativeTlsSecureTransportContextHandle>(context);
}

SSLContextRef secure_transport_context_from_handle(
    DarwinNativeTlsSecureTransportContextHandle context) {
  return reinterpret_cast<SSLContextRef>(context);
}

SSLProtocol secure_transport_protocol(
    DarwinNativeTlsProtocolVersion protocol) {
  switch (protocol) {
  case DarwinNativeTlsProtocolVersion::tls12:
    return kTLSProtocol12;
  }
  return kTLSProtocol12;
}

vpn_engine::ValidationResult configure_tls_context(
    SSLContextRef context, const DarwinNativeTlsSecureTransportApi &api) {
  if (!has_required_secure_transport_api(api)) {
    return handshake_failed(
        "native TLS SecureTransport API table is incomplete");
  }

  const int status = api.set_protocol_version_min(
      handle_from_secure_transport_context(context),
      DarwinNativeTlsProtocolVersion::tls12);
  if (status != noErr) {
    return handshake_failed(
        osstatus_message("SSLSetProtocolVersionMin(TLS 1.2)", status));
  }

  return {};
}

void destroy_darwin_tls_context(DarwinTlsContext *context) {
  if (!context)
    return;

  if (context->context) {
    SSLClose(context->context);
    CFRelease(context->context);
  }
  delete context;
}

OSStatus socket_read_callback(SSLConnectionRef connection, void *data,
                              size_t *data_length) {
  if (!connection || !data || !data_length)
    return errSSLInternal;

  DarwinTlsIo *io = reinterpret_cast<DarwinTlsIo *>(
      const_cast<void *>(connection));
  const std::size_t requested = *data_length;
  *data_length = 0;
  if (requested == 0)
    return noErr;

  ssize_t received = 0;
  do {
    received = recv(io->socket, data, requested, 0);
  } while (received < 0 && errno == EINTR);

  if (received > 0) {
    *data_length = static_cast<std::size_t>(received);
    return noErr;
  }
  if (received == 0)
    return errSSLClosedGraceful;
  if (errno == EAGAIN || errno == EWOULDBLOCK)
    return errSSLWouldBlock;

  return errSSLClosedAbort;
}

OSStatus socket_write_callback(SSLConnectionRef connection, const void *data,
                               size_t *data_length) {
  if (!connection || !data || !data_length)
    return errSSLInternal;

  DarwinTlsIo *io = reinterpret_cast<DarwinTlsIo *>(
      const_cast<void *>(connection));
  const std::size_t requested = *data_length;
  *data_length = 0;
  if (requested == 0)
    return noErr;

  ssize_t sent = 0;
  do {
    sent = send(io->socket, data, requested, 0);
  } while (sent < 0 && errno == EINTR);

  if (sent > 0) {
    *data_length = static_cast<std::size_t>(sent);
    return noErr;
  }
  if (sent == 0)
    return errSSLClosedAbort;
  if (errno == EAGAIN || errno == EWOULDBLOCK)
    return errSSLWouldBlock;

  return errSSLClosedAbort;
}

std::string cf_error_description(CFErrorRef error) {
  if (!error)
    return {};

  ScopedCfRef<CFStringRef> description(CFErrorCopyDescription(error));
  if (!description.get())
    return {};

  char buffer[512] = {};
  if (!CFStringGetCString(description.get(), buffer, sizeof(buffer),
                         kCFStringEncodingUTF8)) {
    return {};
  }

  return std::string(buffer);
}

bool trust_result_allows_connection(SecTrustResultType result) {
  return result == kSecTrustResultUnspecified ||
         result == kSecTrustResultProceed;
}

vpn_engine::ValidationResult
evaluate_server_trust(SecTrustRef trust, const std::string &sni_host) {
  ScopedCfRef<CFStringRef> host(
      CFStringCreateWithCString(kCFAllocatorDefault, sni_host.c_str(),
                                kCFStringEncodingUTF8));
  if (!host.get())
    return verify_failed("failed to create certificate hostname policy");

  ScopedCfRef<SecPolicyRef> policy(SecPolicyCreateSSL(true, host.get()));
  if (!policy.get())
    return verify_failed("failed to create SSL certificate policy");

  OSStatus status = SecTrustSetPolicies(trust, policy.get());
  if (status != errSecSuccess)
    return verify_failed(osstatus_message("SecTrustSetPolicies", status));

  bool trusted = false;
#if defined(__clang__)
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunguarded-availability-new"
#endif
  if (__builtin_available(macOS 10.14, *)) {
    CFErrorRef raw_error = nullptr;
    trusted = SecTrustEvaluateWithError(trust, &raw_error);
    ScopedCfRef<CFErrorRef> error(raw_error);
    if (!trusted) {
      std::string detail = cf_error_description(error.get());
      if (detail.empty())
        detail = "server certificate verification failed";
      return verify_failed(detail);
    }
  } else {
#if defined(__clang__)
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
#endif
    SecTrustResultType trust_result = kSecTrustResultInvalid;
    status = SecTrustEvaluate(trust, &trust_result);
#if defined(__clang__)
#pragma clang diagnostic pop
#endif
    trusted =
        status == errSecSuccess && trust_result_allows_connection(trust_result);
    if (!trusted) {
      std::ostringstream out;
      out << "server certificate verification failed";
      if (status != errSecSuccess)
        out << " with OSStatus " << status;
      else
        out << " with trust result " << trust_result;
      return verify_failed(out.str());
    }
  }
#if defined(__clang__)
#pragma clang diagnostic pop
#endif

  return {};
}

vpn_engine::ValidationResult
verify_server_certificate(SSLContextRef context, const std::string &sni_host) {
  ScopedCfRef<SecTrustRef> trust;
  OSStatus status = SSLCopyPeerTrust(context, trust.out());
  if (status != noErr || !trust.get()) {
    return verify_failed(
        "failed to read server certificate trust from TLS context");
  }

  return evaluate_server_trust(trust.get(), sni_host);
}

bool tls_status_is_certificate_failure(OSStatus status) {
  switch (status) {
  case errSSLXCertChainInvalid:
  case errSSLBadCert:
  case errSSLUnknownRootCert:
  case errSSLNoRootCert:
  case errSSLCertExpired:
  case errSSLCertNotYetValid:
  case errSSLPeerBadCert:
  case errSSLPeerUnsupportedCert:
  case errSSLPeerCertRevoked:
  case errSSLPeerCertExpired:
  case errSSLPeerCertUnknown:
  case errSSLPeerUnknownCA:
  case errSSLHostNameMismatch:
    return true;
  default:
    return false;
  }
}
// End inlined from platform/darwin/native_tls_stream_context include-unit
// Begin inlined from platform/darwin/native_tls_stream_api include-unit
class DarwinNativeTlsApiImpl final : public DarwinNativeTlsApi {
public:
  explicit DarwinNativeTlsApiImpl(DarwinNativeTlsDependencies dependencies)
      : dependencies_(std::move(dependencies)) {}

  DarwinNativeTlsTcpConnectResult connect_tcp(
      const vpn_engine::protocol::TlsEndpoint &endpoint) override {
    DarwinNativeTlsTcpConnectResult result;
    if (endpoint.host.empty()) {
      result.result = connect_failed("TLS endpoint host is not configured");
      return result;
    }

    if (!has_required_tcp_api(dependencies_.tcp)) {
      result.result =
          connect_failed("native TLS TCP API table is incomplete");
      return result;
    }

    DarwinNativeTlsTcpConnectResult connected =
        dependencies_.tcp.open_connected_socket(endpoint, kConnectTimeoutMs);
    if (!connected.result.ok)
      return connected;
    if (!valid_socket_handle(connected.socket)) {
      result.result = connect_failed("TCP connect did not return a socket");
      return result;
    }

    vpn_engine::ValidationResult configured =
        configure_socket(connected.socket, dependencies_.socket_options);
    if (!configured.ok) {
      dependencies_.tcp.close_socket(connected.socket);
      result.result = configured;
      return result;
    }

    result.socket = connected.socket;
    return result;
  }

  DarwinNativeTlsHandshakeResult
  handshake(DarwinNativeTlsSocketHandle socket,
            const std::string &sni_host) override {
    DarwinNativeTlsHandshakeResult result;

    std::unique_ptr<DarwinTlsContext, decltype(&destroy_darwin_tls_context)>
        context(new DarwinTlsContext(), destroy_darwin_tls_context);
    context->io.socket = socket;
    context->context =
        SSLCreateContext(kCFAllocatorDefault, kSSLClientSide, kSSLStreamType);
    if (!context->context) {
      result.result = handshake_failed("failed to create SecureTransport context");
      return result;
    }

    vpn_engine::ValidationResult configured =
        configure_tls_context(context->context, dependencies_.secure_transport);
    if (!configured.ok) {
      result.result = configured;
      return result;
    }

    OSStatus status = SSLSetIOFuncs(context->context, socket_read_callback,
                                    socket_write_callback);
    if (status != noErr) {
      result.result = handshake_failed(osstatus_message("SSLSetIOFuncs", status));
      return result;
    }

    status = SSLSetConnection(
        context->context,
        reinterpret_cast<SSLConnectionRef>(&context->io));
    if (status != noErr) {
      result.result =
          handshake_failed(osstatus_message("SSLSetConnection", status));
      return result;
    }

    status =
        SSLSetPeerDomainName(context->context, sni_host.data(), sni_host.size());
    if (status != noErr) {
      result.result =
          handshake_failed(osstatus_message("SSLSetPeerDomainName", status));
      return result;
    }

    status = SSLSetSessionOption(context->context,
                                 kSSLSessionOptionBreakOnServerAuth, true);
    if (status != noErr) {
      result.result = handshake_failed(
          osstatus_message("SSLSetSessionOption(BreakOnServerAuth)", status));
      return result;
    }

    bool trust_verified = false;
    for (int step = 0; step < kMaxHandshakeSteps; ++step) {
      status = SSLHandshake(context->context);

      if (status == noErr) {
        if (!trust_verified) {
          vpn_engine::ValidationResult verified =
              verify_server_certificate(context->context, sni_host);
          if (!verified.ok) {
            result.tls_context = handle_from_context(context.release());
            result.result = verified;
            return result;
          }
        }

        result.tls_context = handle_from_context(context.release());
        return result;
      }

      if (status == errSSLWouldBlock)
        continue;

      if (status == errSSLServerAuthCompleted) {
        vpn_engine::ValidationResult verified =
            verify_server_certificate(context->context, sni_host);
        if (!verified.ok) {
          result.tls_context = handle_from_context(context.release());
          result.result = verified;
          return result;
        }

        trust_verified = true;
        continue;
      }

      result.tls_context = handle_from_context(context.release());
      result.result =
          tls_status_is_certificate_failure(status)
              ? verify_failed(osstatus_message("SSLHandshake", status))
              : handshake_failed(osstatus_message("SSLHandshake", status));
      return result;
    }

    result.tls_context = handle_from_context(context.release());
    result.result = handshake_failed("TLS handshake exceeded step limit");
    return result;
  }

  vpn_engine::ValidationResult
  write_plaintext(DarwinNativeTlsContextHandle tls_context,
                  DarwinNativeTlsSocketHandle,
                  const std::vector<std::uint8_t> &bytes) override {
    if (bytes.empty())
      return {};

    DarwinTlsContext *context = context_from_handle(tls_context);
    if (!context || !context->context) {
      return invalid("tls_stream_not_connected", "TLS context is not connected");
    }

    std::size_t offset = 0;
    while (offset < bytes.size()) {
      const std::size_t remaining = bytes.size() - offset;
      const std::size_t chunk = std::min<std::size_t>(
          remaining, static_cast<std::size_t>(std::numeric_limits<int>::max()));
      std::size_t processed = 0;
      OSStatus status =
          SSLWrite(context->context, bytes.data() + offset, chunk, &processed);

      if (processed > 0) {
        offset += processed;
        continue;
      }

      if (status == noErr)
        return invalid("tls_write_failed", "TLS write made no progress");
      if (status == errSSLWouldBlock)
        return invalid("tls_write_failed", "TLS write timed out or would block");
      if (status == errSSLClosedGraceful || status == errSSLClosedAbort ||
          status == errSSLClosedNoNotify) {
        return invalid("transport_closed", "TLS peer closed during write");
      }

      return invalid("tls_write_failed", osstatus_message("SSLWrite", status));
    }

    return {};
  }

  DarwinNativeTlsReadResult
  read_plaintext(DarwinNativeTlsContextHandle tls_context,
                 DarwinNativeTlsSocketHandle) override {
    DarwinNativeTlsReadResult result;
    DarwinTlsContext *context = context_from_handle(tls_context);
    if (!context || !context->context) {
      result.result =
          invalid("tls_stream_not_connected", "TLS context is not connected");
      return result;
    }

    std::vector<std::uint8_t> buffer(kPlaintextReadSize);
    std::size_t processed = 0;
    OSStatus status =
        SSLRead(context->context, buffer.data(), buffer.size(), &processed);

    if (processed > 0) {
      result.bytes.assign(buffer.begin(), buffer.begin() + processed);
      return result;
    }

    if (status == errSSLClosedGraceful || status == errSSLClosedAbort ||
        status == errSSLClosedNoNotify) {
      result.peer_closed = true;
      return result;
    }
    if (status == noErr) {
      result.result = invalid("tls_read_failed", "TLS read returned no data");
      return result;
    }
    if (status == errSSLWouldBlock) {
      result.result =
          invalid("tls_read_failed", "TLS read timed out or would block");
      return result;
    }

    result.result =
        invalid("tls_read_failed", osstatus_message("SSLRead", status));
    return result;
  }

  void close_tls_context(DarwinNativeTlsContextHandle tls_context) override {
    destroy_darwin_tls_context(context_from_handle(tls_context));
  }

  void close_socket(DarwinNativeTlsSocketHandle socket) override {
    if (!valid_socket_handle(socket))
      return;
    if (dependencies_.tcp.close_socket)
      dependencies_.tcp.close_socket(socket);
    else
      ::close(socket);
  }

private:
  DarwinNativeTlsDependencies dependencies_;
};
// End inlined from platform/darwin/native_tls_stream_api include-unit
} // namespace
// Begin inlined from platform/darwin/native_tls_stream_deps include-unit
DarwinNativeTlsDependencies default_darwin_native_tls_dependencies() {
  DarwinNativeTlsDependencies dependencies;

  dependencies.tcp.open_connected_socket =
      [](const vpn_engine::protocol::TlsEndpoint &endpoint, int timeout_ms) {
        DarwinNativeTlsTcpConnectResult result;

        addrinfo hints{};
        hints.ai_family = AF_UNSPEC;
        hints.ai_socktype = SOCK_STREAM;
        hints.ai_protocol = IPPROTO_TCP;

        addrinfo *raw_addresses = nullptr;
        const std::string port = std::to_string(endpoint.port);
        const int resolved =
            getaddrinfo(endpoint.host.c_str(), port.c_str(), &hints,
                        &raw_addresses);
        std::unique_ptr<addrinfo, AddrInfoDeleter> addresses(raw_addresses);
        if (resolved != 0) {
          result.result =
              connect_failed("DNS resolution failed for " + endpoint.host);
          return result;
        }

        int last_error = 0;
        for (addrinfo *address = addresses.get(); address;
             address = address->ai_next) {
          int socket = ::socket(address->ai_family, address->ai_socktype,
                                address->ai_protocol);
          if (socket < 0) {
            last_error = errno;
            continue;
          }

          const bool connected = connect_socket_with_timeout(
              socket, address->ai_addr,
              static_cast<socklen_t>(address->ai_addrlen), timeout_ms);
          if (connected) {
            result.socket = socket;
            return result;
          }

          last_error = errno;
          ::close(socket);
        }

        if (last_error != 0) {
          result.result =
              connect_failed(errno_message("TCP connect", last_error));
        } else {
          result.result = connect_failed(
              "TCP connect failed or timed out for " + endpoint.host);
        }
        return result;
      };

  dependencies.tcp.close_socket = [](DarwinNativeTlsSocketHandle socket) {
    if (valid_socket_handle(socket))
      ::close(socket);
  };

  dependencies.socket_options.set_socket_option =
      [](DarwinNativeTlsSocketHandle socket, int level, int option,
         const void *value, std::size_t value_size) {
        return setsockopt(socket, level, option, value,
                          static_cast<socklen_t>(value_size));
      };
  dependencies.socket_options.last_error = [] { return errno; };

  dependencies.secure_transport.set_protocol_version_min =
      [](DarwinNativeTlsSecureTransportContextHandle context,
         DarwinNativeTlsProtocolVersion protocol) {
        return SSLSetProtocolVersionMin(secure_transport_context_from_handle(
                                            context),
                                        secure_transport_protocol(protocol));
      };

  return dependencies;
}

std::unique_ptr<DarwinNativeTlsApi> make_darwin_native_tls_api() {
  return make_darwin_native_tls_api(default_darwin_native_tls_dependencies());
}

std::unique_ptr<DarwinNativeTlsApi>
make_darwin_native_tls_api(DarwinNativeTlsDependencies dependencies) {
  return std::unique_ptr<DarwinNativeTlsApi>(
      new DarwinNativeTlsApiImpl(std::move(dependencies)));
}
// End inlined from platform/darwin/native_tls_stream_deps include-unit
// Begin inlined from platform/darwin/native_tls_stream_public include-unit
NativeTlsStream::NativeTlsStream()
    : NativeTlsStream(make_darwin_native_tls_api()) {}

NativeTlsStream::NativeTlsStream(std::unique_ptr<DarwinNativeTlsApi> api)
    : api_(std::move(api)) {}

NativeTlsStream::~NativeTlsStream() { close(); }

vpn_engine::ValidationResult NativeTlsStream::connect(
    const vpn_engine::protocol::TlsEndpoint &endpoint) {
  close();
  closed_ = false;

  if (!api_)
    return invalid("tls_api_missing", "native TLS API is not configured");

  const std::string verification_host =
      endpoint.sni_host.empty() ? endpoint.host : endpoint.sni_host;
  if (verification_host.empty()) {
    return invalid("tls_endpoint_invalid",
                   "TLS endpoint host or SNI host must be configured");
  }

  DarwinNativeTlsTcpConnectResult tcp = api_->connect_tcp(endpoint);
  if (valid_socket_handle(tcp.socket))
    socket_ = tcp.socket;
  if (!tcp.result.ok)
    return fail_connect_and_reset(tcp.result);
  if (!valid_socket_handle(socket_)) {
    return fail_connect_and_reset(
        connect_failed("TCP connect did not return a socket"));
  }

  DarwinNativeTlsHandshakeResult tls =
      api_->handshake(socket_, verification_host);
  if (valid_context_handle(tls.tls_context))
    tls_context_ = tls.tls_context;
  if (!tls.result.ok)
    return fail_connect_and_reset(tls.result);
  if (!valid_context_handle(tls_context_)) {
    return fail_connect_and_reset(
        handshake_failed("TLS handshake did not return a security context"));
  }

  connected_ = true;
  closed_ = false;
  return {};
}

vpn_engine::ValidationResult
NativeTlsStream::write_all(const std::vector<std::uint8_t> &bytes) {
  vpn_engine::ValidationResult open = ensure_open();
  if (!open.ok)
    return open;
  if (bytes.empty())
    return {};

  vpn_engine::ValidationResult written =
      api_->write_plaintext(tls_context_, socket_, bytes);
  if (!written.ok)
    return fail_and_close(written);

  return {};
}

vpn_engine::ValidationResult
NativeTlsStream::read_some(std::vector<std::uint8_t> *bytes) {
  if (!bytes)
    return invalid("tls_stream_null_output", "read output must not be null");

  vpn_engine::ValidationResult open = ensure_open();
  if (!open.ok)
    return open;

  bytes->clear();

  DarwinNativeTlsReadResult read = api_->read_plaintext(tls_context_, socket_);
  if (!read.result.ok)
    return fail_and_close(read.result);
  if (read.peer_closed) {
    close();
    return {};
  }
  if (read.bytes.empty()) {
    return fail_and_close(
        invalid("tls_read_failed", "TLS read returned no data"));
  }

  *bytes = std::move(read.bytes);
  return {};
}

void NativeTlsStream::close() {
  const DarwinNativeTlsContextHandle tls_context = tls_context_;
  const DarwinNativeTlsSocketHandle socket = socket_;

  tls_context_ = kInvalidDarwinNativeTlsContextHandle;
  socket_ = kInvalidDarwinNativeTlsSocketHandle;
  connected_ = false;
  closed_ = true;

  if (!api_)
    return;

  if (valid_context_handle(tls_context))
    api_->close_tls_context(tls_context);
  if (valid_socket_handle(socket))
    api_->close_socket(socket);
}

vpn_engine::ValidationResult NativeTlsStream::ensure_open() const {
  if (closed_)
    return invalid("tls_stream_closed", "TLS stream is closed");
  if (!connected_)
    return invalid("tls_stream_not_connected", "TLS stream is not connected");
  return {};
}

vpn_engine::ValidationResult
NativeTlsStream::fail_and_close(vpn_engine::ValidationResult result) {
  close();
  return result;
}

vpn_engine::ValidationResult
NativeTlsStream::fail_connect_and_reset(vpn_engine::ValidationResult result) {
  close();
  closed_ = false;
  return result;
}
// End inlined from platform/darwin/native_tls_stream_public include-unit
} // namespace platform
} // namespace exv
