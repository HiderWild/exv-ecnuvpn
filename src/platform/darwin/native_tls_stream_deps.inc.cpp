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
