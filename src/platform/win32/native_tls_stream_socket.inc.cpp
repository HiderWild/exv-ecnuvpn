void append_bytes(std::vector<std::uint8_t> *target,
                  const std::vector<std::uint8_t> &source) {
  target->insert(target->end(), source.begin(), source.end());
}

vpn_engine::ValidationResult send_all_socket(SOCKET socket,
                                             const std::uint8_t *data,
                                             std::size_t size) {
  std::size_t sent = 0;
  while (sent < size) {
    const std::size_t remaining = size - sent;
    const int chunk = static_cast<int>(
        std::min<std::size_t>(remaining, static_cast<std::size_t>(
                                             std::numeric_limits<int>::max())));
    const int written =
        ::send(socket, reinterpret_cast<const char *>(data + sent), chunk, 0);
    if (written == SOCKET_ERROR) {
      return invalid("tls_write_failed",
                     wsa_error_message("send", WSAGetLastError()));
    }
    if (written == 0) {
      return invalid("transport_closed", "socket closed while sending TLS data");
    }
    sent += static_cast<std::size_t>(written);
  }

  return {};
}

bool wait_for_connect(SOCKET socket, int timeout_ms) {
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
      select(0, nullptr, &write_set, &error_set, &timeout);
  if (selected <= 0)
    return false;

  int socket_error = 0;
  int socket_error_size = sizeof(socket_error);
  if (getsockopt(socket, SOL_SOCKET, SO_ERROR,
                 reinterpret_cast<char *>(&socket_error),
                 &socket_error_size) == SOCKET_ERROR) {
    return false;
  }

  return socket_error == 0;
}

bool connect_socket_with_timeout(SOCKET socket, const sockaddr *address,
                                 int address_length, int timeout_ms) {
  u_long nonblocking = 1;
  if (ioctlsocket(socket, FIONBIO, &nonblocking) == SOCKET_ERROR)
    return false;

  const int connected = ::connect(socket, address, address_length);
  if (connected == 0) {
    u_long blocking = 0;
    ioctlsocket(socket, FIONBIO, &blocking);
    return true;
  }

  const int error = WSAGetLastError();
  if (error != WSAEWOULDBLOCK && error != WSAEINPROGRESS &&
      error != WSAEINVAL) {
    return false;
  }

  if (!wait_for_connect(socket, timeout_ms))
    return false;

  u_long blocking = 0;
  if (ioctlsocket(socket, FIONBIO, &blocking) == SOCKET_ERROR)
    return false;

  return true;
}

void configure_socket_timeouts(SOCKET socket) {
  DWORD timeout = static_cast<DWORD>(kConnectTimeoutMs);
  setsockopt(socket, SOL_SOCKET, SO_RCVTIMEO,
             reinterpret_cast<const char *>(&timeout), sizeof(timeout));
  setsockopt(socket, SOL_SOCKET, SO_SNDTIMEO,
             reinterpret_cast<const char *>(&timeout), sizeof(timeout));
}

