class WindowsNativeTlsApi final : public NativeTlsApi {
public:
  vpn_engine::ValidationResult startup() override {
    WSADATA data;
    const int started = WSAStartup(MAKEWORD(2, 2), &data);
    if (started != 0)
      return connect_failed(wsa_error_message("WSAStartup", started));
    return {};
  }

  NativeTlsTcpConnectResult connect_tcp(
      const vpn_engine::protocol::TlsEndpoint &endpoint) override {
    NativeTlsTcpConnectResult result;

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

    for (addrinfo *address = addresses.get(); address;
         address = address->ai_next) {
      SOCKET socket =
          ::socket(address->ai_family, address->ai_socktype,
                   address->ai_protocol);
      if (socket == INVALID_SOCKET)
        continue;

      if (connect_socket_with_timeout(
              socket, address->ai_addr,
              static_cast<int>(address->ai_addrlen), kConnectTimeoutMs)) {
        configure_socket_timeouts(socket);
        result.socket = handle_from_socket(socket);
        return result;
      }

      closesocket(socket);
    }

    result.result =
        connect_failed("TCP connect failed or timed out for " + endpoint.host);
    return result;
  }

  NativeTlsHandshakeResult handshake(NativeTlsSocketHandle socket_handle,
                                     const std::string &sni_host) override {
    NativeTlsHandshakeResult result;
    SOCKET socket = socket_from_handle(socket_handle);

    SchannelContextPtr context(new SchannelTlsContext(),
                               destroy_schannel_context);

    SCHANNEL_CRED credentials{};
    credentials.dwVersion = SCHANNEL_CRED_VERSION;
    credentials.dwFlags = SCH_CRED_MANUAL_CRED_VALIDATION |
                          SCH_CRED_NO_DEFAULT_CREDS | SCH_USE_STRONG_CRYPTO;

    TimeStamp expiry{};
    SECURITY_STATUS status = AcquireCredentialsHandleA(
        nullptr, const_cast<SEC_CHAR *>(UNISP_NAME_A), SECPKG_CRED_OUTBOUND,
        nullptr, &credentials, nullptr, nullptr, &context->credentials,
        &expiry);
    if (status != SEC_E_OK) {
      result.result = handshake_failed(
          "AcquireCredentialsHandle failed with " + security_status_hex(status));
      return result;
    }
    context->has_credentials = true;

    std::vector<std::uint8_t> incoming;
    const unsigned long request_flags =
        ISC_REQ_ALLOCATE_MEMORY | ISC_REQ_CONFIDENTIALITY |
        ISC_REQ_REPLAY_DETECT | ISC_REQ_SEQUENCE_DETECT | ISC_REQ_STREAM |
        ISC_REQ_EXTENDED_ERROR | ISC_REQ_MANUAL_CRED_VALIDATION;

    for (int step = 0; step < kMaxHandshakeSteps; ++step) {
      SecBuffer output_buffer{};
      output_buffer.BufferType = SECBUFFER_TOKEN;

      SecBufferDesc output_desc{};
      output_desc.ulVersion = SECBUFFER_VERSION;
      output_desc.cBuffers = 1;
      output_desc.pBuffers = &output_buffer;

      SecBuffer input_buffers[2]{};
      SecBufferDesc input_desc{};
      SecBufferDesc *input_desc_ptr = nullptr;

      if (!incoming.empty()) {
        input_buffers[0].BufferType = SECBUFFER_TOKEN;
        input_buffers[0].pvBuffer = incoming.data();
        input_buffers[0].cbBuffer =
            static_cast<unsigned long>(incoming.size());
        input_buffers[1].BufferType = SECBUFFER_EMPTY;

        input_desc.ulVersion = SECBUFFER_VERSION;
        input_desc.cBuffers = 2;
        input_desc.pBuffers = input_buffers;
        input_desc_ptr = &input_desc;
      }

      unsigned long attributes = 0;
      status = InitializeSecurityContextA(
          &context->credentials,
          context->has_context ? &context->context : nullptr,
          const_cast<SEC_CHAR *>(sni_host.c_str()), request_flags, 0,
          SECURITY_NATIVE_DREP, input_desc_ptr, 0, &context->context,
          &output_desc, &attributes, &expiry);

      if (status == SEC_E_OK || status == SEC_I_CONTINUE_NEEDED)
        context->has_context = true;

      const bool have_output = output_buffer.pvBuffer &&
                               output_buffer.cbBuffer > 0;
      if (have_output) {
        vpn_engine::ValidationResult sent = send_all_socket(
            socket, static_cast<const std::uint8_t *>(output_buffer.pvBuffer),
            static_cast<std::size_t>(output_buffer.cbBuffer));
        FreeContextBuffer(output_buffer.pvBuffer);
        if (!sent.ok) {
          result.result = sent;
          return result;
        }
      } else if (output_buffer.pvBuffer) {
        FreeContextBuffer(output_buffer.pvBuffer);
      }

      if (status == SEC_E_OK) {
        if (input_desc_ptr &&
            input_buffers[1].BufferType == SECBUFFER_EXTRA &&
            input_buffers[1].cbBuffer <= incoming.size()) {
          const std::size_t extra_size =
              static_cast<std::size_t>(input_buffers[1].cbBuffer);
          result.encrypted_extra.assign(incoming.end() - extra_size,
                                        incoming.end());
        }

        status = QueryContextAttributesA(
            &context->context, SECPKG_ATTR_STREAM_SIZES, &context->sizes);
        if (status != SEC_E_OK) {
          result.result = handshake_failed(
              "QueryContextAttributes(stream sizes) failed with " +
              security_status_hex(status));
          return result;
        }

        vpn_engine::ValidationResult verified =
            verify_server_certificate(context.get(), sni_host);
        result.tls_context = handle_from_context(context.get());
        if (!verified.ok) {
          result.result = verified;
          context.release();
          return result;
        }

        context.release();
        return result;
      }

      if (status == SEC_E_INCOMPLETE_MESSAGE) {
        vpn_engine::ValidationResult failure;
        if (!receive_handshake_bytes(socket, &incoming, &failure)) {
          result.result = failure;
          return result;
        }
        continue;
      }

      if (status == SEC_I_CONTINUE_NEEDED) {
        if (input_desc_ptr)
          keep_handshake_extra(&incoming, input_buffers[1]);

        if (incoming.empty()) {
          vpn_engine::ValidationResult failure;
          if (!receive_handshake_bytes(socket, &incoming, &failure)) {
            result.result = failure;
            return result;
          }
        }
        continue;
      }

      result.result = handshake_failed(
          "InitializeSecurityContext failed with " +
          security_status_hex(status));
      return result;
    }

    result.result = handshake_failed("TLS handshake exceeded step limit");
    return result;
  }

  vpn_engine::ValidationResult send_plaintext(
      NativeTlsContextHandle tls_context, NativeTlsSocketHandle socket_handle,
      const std::vector<std::uint8_t> &bytes) override {
    if (bytes.empty())
      return {};

    SchannelTlsContext *context = context_from_handle(tls_context);
    if (!context || !context->has_context)
      return invalid("tls_stream_not_connected", "TLS context is not connected");

    SOCKET socket = socket_from_handle(socket_handle);
    const std::size_t max_message = std::max<std::size_t>(
        1, static_cast<std::size_t>(context->sizes.cbMaximumMessage));
    std::size_t offset = 0;

    while (offset < bytes.size()) {
      const std::size_t chunk_size =
          std::min(max_message, bytes.size() - offset);
      std::vector<std::uint8_t> packet(
          static_cast<std::size_t>(context->sizes.cbHeader) + chunk_size +
          static_cast<std::size_t>(context->sizes.cbTrailer));

      std::memcpy(packet.data() + context->sizes.cbHeader,
                  bytes.data() + offset, chunk_size);

      SecBuffer buffers[4]{};
      buffers[0].BufferType = SECBUFFER_STREAM_HEADER;
      buffers[0].pvBuffer = packet.data();
      buffers[0].cbBuffer = context->sizes.cbHeader;
      buffers[1].BufferType = SECBUFFER_DATA;
      buffers[1].pvBuffer = packet.data() + context->sizes.cbHeader;
      buffers[1].cbBuffer = static_cast<unsigned long>(chunk_size);
      buffers[2].BufferType = SECBUFFER_STREAM_TRAILER;
      buffers[2].pvBuffer =
          packet.data() + context->sizes.cbHeader + chunk_size;
      buffers[2].cbBuffer = context->sizes.cbTrailer;
      buffers[3].BufferType = SECBUFFER_EMPTY;

      SecBufferDesc desc{};
      desc.ulVersion = SECBUFFER_VERSION;
      desc.cBuffers = 4;
      desc.pBuffers = buffers;

      SECURITY_STATUS status = EncryptMessage(&context->context, 0, &desc, 0);
      if (status != SEC_E_OK) {
        return invalid("tls_write_failed",
                       "EncryptMessage failed with " +
                           security_status_hex(status));
      }

      const std::size_t encrypted_size =
          static_cast<std::size_t>(buffers[0].cbBuffer) +
          static_cast<std::size_t>(buffers[1].cbBuffer) +
          static_cast<std::size_t>(buffers[2].cbBuffer);
      vpn_engine::ValidationResult sent =
          send_all_socket(socket, packet.data(), encrypted_size);
      if (!sent.ok)
        return sent;

      offset += chunk_size;
    }

    return {};
  }

  NativeTlsRecvResult
  recv_encrypted(NativeTlsSocketHandle socket_handle) override {
    NativeTlsRecvResult result;
    std::vector<std::uint8_t> buffer(kEncryptedReadSize);
    const int received =
        recv(socket_from_handle(socket_handle),
             reinterpret_cast<char *>(buffer.data()),
             static_cast<int>(buffer.size()), 0);
    if (received == 0) {
      result.peer_closed = true;
      return result;
    }
    if (received == SOCKET_ERROR) {
      result.result =
          invalid("tls_read_failed",
                  wsa_error_message("recv", WSAGetLastError()));
      return result;
    }

    result.bytes.assign(buffer.begin(), buffer.begin() + received);
    return result;
  }

  NativeTlsDecryptResult decrypt(
      NativeTlsContextHandle tls_context,
      const std::vector<std::uint8_t> &encrypted) override {
    NativeTlsDecryptResult result;
    SchannelTlsContext *context = context_from_handle(tls_context);
    if (!context || !context->has_context) {
      result.result =
          invalid("tls_stream_not_connected", "TLS context is not connected");
      return result;
    }

    if (encrypted.empty()) {
      result.status = NativeTlsReadStatus::need_more_data;
      return result;
    }

    std::vector<std::uint8_t> buffer = encrypted;
    SecBuffer buffers[4]{};
    buffers[0].BufferType = SECBUFFER_DATA;
    buffers[0].pvBuffer = buffer.data();
    buffers[0].cbBuffer = static_cast<unsigned long>(buffer.size());
    buffers[1].BufferType = SECBUFFER_EMPTY;
    buffers[2].BufferType = SECBUFFER_EMPTY;
    buffers[3].BufferType = SECBUFFER_EMPTY;

    SecBufferDesc desc{};
    desc.ulVersion = SECBUFFER_VERSION;
    desc.cBuffers = 4;
    desc.pBuffers = buffers;

    unsigned long qop = 0;
    SECURITY_STATUS status =
        DecryptMessage(&context->context, &desc, 0, &qop);
    if (status == SEC_E_INCOMPLETE_MESSAGE) {
      result.status = NativeTlsReadStatus::need_more_data;
      return result;
    }
    if (status == SEC_I_CONTEXT_EXPIRED) {
      result.status = NativeTlsReadStatus::peer_closed;
      return result;
    }
    if (status != SEC_E_OK) {
      result.status = NativeTlsReadStatus::tls_alert;
      result.result =
          invalid("tls_alert",
                  "DecryptMessage failed with " + security_status_hex(status));
      return result;
    }

    std::size_t extra_size = 0;
    for (SecBuffer &sec_buffer : buffers) {
      if (sec_buffer.BufferType == SECBUFFER_DATA && sec_buffer.pvBuffer &&
          sec_buffer.cbBuffer > 0) {
        const auto *data =
            static_cast<const std::uint8_t *>(sec_buffer.pvBuffer);
        result.plaintext.assign(data, data + sec_buffer.cbBuffer);
      } else if (sec_buffer.BufferType == SECBUFFER_EXTRA) {
        extra_size = static_cast<std::size_t>(sec_buffer.cbBuffer);
      }
    }

    if (extra_size > encrypted.size()) {
      result.result = invalid("tls_decrypt_failed",
                              "Schannel returned invalid extra byte count");
      return result;
    }

    result.encrypted_bytes_consumed = encrypted.size() - extra_size;
    result.status = NativeTlsReadStatus::data;
    return result;
  }

  void close_tls_context(NativeTlsContextHandle tls_context) override {
    destroy_schannel_context(context_from_handle(tls_context));
  }

  void close_socket(NativeTlsSocketHandle socket) override {
    if (valid_socket_handle(socket))
      closesocket(socket_from_handle(socket));
  }

  void shutdown() override { WSACleanup(); }
};
