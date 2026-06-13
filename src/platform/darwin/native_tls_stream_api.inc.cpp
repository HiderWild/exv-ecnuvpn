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
