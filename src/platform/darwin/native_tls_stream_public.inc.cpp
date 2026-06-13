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
