std::unique_ptr<NativeTlsApi> make_windows_native_tls_api() {
  return std::unique_ptr<NativeTlsApi>(new WindowsNativeTlsApi());
}

NativeTlsStream::NativeTlsStream()
    : NativeTlsStream(make_windows_native_tls_api()) {}

NativeTlsStream::NativeTlsStream(std::unique_ptr<NativeTlsApi> api)
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

  vpn_engine::ValidationResult started = api_->startup();
  if (!started.ok)
    return started;
  api_started_ = true;

  NativeTlsTcpConnectResult tcp = api_->connect_tcp(endpoint);
  if (valid_socket_handle(tcp.socket))
    socket_ = tcp.socket;
  if (!tcp.result.ok)
    return fail_connect_and_reset(tcp.result);
  if (!valid_socket_handle(socket_)) {
    return fail_connect_and_reset(
        connect_failed("TCP connect did not return a socket"));
  }

  NativeTlsHandshakeResult tls = api_->handshake(socket_, verification_host);
  if (valid_context_handle(tls.tls_context))
    tls_context_ = tls.tls_context;
  if (!tls.result.ok)
    return fail_connect_and_reset(tls.result);
  if (!valid_context_handle(tls_context_)) {
    return fail_connect_and_reset(
        handshake_failed("TLS handshake did not return a security context"));
  }

  encrypted_buffer_ = std::move(tls.encrypted_extra);
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
      api_->send_plaintext(tls_context_, socket_, bytes);
  if (!written.ok)
    return fail_and_close(written);

  return {};
}

vpn_engine::ValidationResult
NativeTlsStream::read_some(std::vector<std::uint8_t> *bytes) {
  if (!bytes)
    return invalid("tls_stream_null_output", "read output must not be null");

  {
    std::lock_guard<std::mutex> lock(io_mutex_);
    vpn_engine::ValidationResult open = ensure_open();
    if (!open.ok)
      return open;
  }

  bytes->clear();

  // Full-duplex hardening: the decrypt path and the buffer mutate under
  // io_mutex_ so close() (driven from the outbound thread) can never destroy
  // the Schannel context while this reader thread is mid-DecryptMessage. The
  // blocking recv() runs OUTSIDE the lock; close() interrupts it by closing the
  // socket, after which the reader re-checks closed_ under the lock and bails
  // before touching the torn-down context.
  enum class Step { return_ok, loop, do_recv, peer_closed, fail };

  while (true) {
    Step step = Step::do_recv;
    vpn_engine::ValidationResult fail_result;
    NativeTlsSocketHandle recv_socket = kInvalidNativeTlsSocketHandle;

    {
      std::lock_guard<std::mutex> lock(io_mutex_);
      if (closed_.load())
        return invalid("tls_stream_closed", "TLS stream is closed");

      if (encrypted_buffer_.empty()) {
        step = Step::do_recv;
        recv_socket = socket_;
      } else {
        NativeTlsDecryptResult decrypted =
            api_->decrypt(tls_context_, encrypted_buffer_);
        if (!decrypted.result.ok) {
          step = Step::fail;
          fail_result = decrypted.result;
        } else if (decrypted.encrypted_bytes_consumed >
                   encrypted_buffer_.size()) {
          step = Step::fail;
          fail_result = invalid(
              "tls_decrypt_failed",
              "TLS decrypt consumed more encrypted bytes than available");
        } else {
          const std::size_t consumed = decrypted.encrypted_bytes_consumed;
          if (consumed > 0) {
            encrypted_buffer_.erase(encrypted_buffer_.begin(),
                                    encrypted_buffer_.begin() + consumed);
          }

          switch (decrypted.status) {
          case NativeTlsReadStatus::data:
            if (!decrypted.plaintext.empty()) {
              if (consumed == 0) {
                step = Step::fail;
                fail_result = invalid(
                    "tls_decrypt_failed",
                    "TLS decrypt returned plaintext without consuming input");
              } else {
                *bytes = std::move(decrypted.plaintext);
                step = Step::return_ok;
              }
            } else if (consumed == 0) {
              step = Step::fail;
              fail_result =
                  invalid("tls_decrypt_failed", "TLS decrypt made no progress");
            } else {
              step = Step::loop;
            }
            break;

          case NativeTlsReadStatus::need_more_data:
            step = Step::do_recv;
            recv_socket = socket_;
            break;

          case NativeTlsReadStatus::peer_closed:
            step = Step::peer_closed;
            break;

          case NativeTlsReadStatus::tls_alert:
            step = Step::fail;
            fail_result = invalid("tls_alert", "TLS alert received");
            break;
          }
        }
      }
    }

    switch (step) {
    case Step::return_ok:
      return {};
    case Step::loop:
      continue;
    case Step::peer_closed:
      close();
      return {};
    case Step::fail:
      return fail_and_close(fail_result);
    case Step::do_recv:
      break;
    }

    // Blocking recv must stay outside io_mutex_ so close() can interrupt it.
    NativeTlsRecvResult received = api_->recv_encrypted(recv_socket);
    if (!received.result.ok)
      return fail_and_close(received.result);
    if (received.peer_closed) {
      close();
      return {};
    }
    if (received.bytes.empty())
      return fail_and_close(
          invalid("tls_read_failed", "socket read returned no data"));

    {
      std::lock_guard<std::mutex> lock(io_mutex_);
      if (closed_.load())
        return invalid("tls_stream_closed", "TLS stream is closed");
      append_bytes(&encrypted_buffer_, received.bytes);
    }
  }
}

void NativeTlsStream::close() {
  NativeTlsContextHandle tls_context;
  NativeTlsSocketHandle socket;
  bool api_started;

  {
    std::lock_guard<std::mutex> lock(io_mutex_);
    tls_context = tls_context_;
    socket = socket_;
    api_started = api_started_;

    tls_context_ = kInvalidNativeTlsContextHandle;
    socket_ = kInvalidNativeTlsSocketHandle;
    encrypted_buffer_.clear();
    connected_ = false;
    closed_.store(true);
    api_started_ = false;
  }

  if (!api_)
    return;

  // Destroyed after releasing io_mutex_: any concurrent reader either holds the
  // lock during DecryptMessage (so this teardown waited for it) or re-checks
  // closed_ under the lock before reusing the context. Closing the socket
  // unblocks an in-flight recv() on the reader thread.
  if (valid_context_handle(tls_context))
    api_->close_tls_context(tls_context);
  if (valid_socket_handle(socket))
    api_->close_socket(socket);
  if (api_started)
    api_->shutdown();
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
