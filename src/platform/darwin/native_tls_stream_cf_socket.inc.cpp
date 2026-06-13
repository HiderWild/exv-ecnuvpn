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

