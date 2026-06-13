struct AddrInfoDeleter {
  void operator()(addrinfo *info) const {
    if (info)
      freeaddrinfo(info);
  }
};

struct SchannelTlsContext {
  CredHandle credentials{};
  CtxtHandle context{};
  SecPkgContext_StreamSizes sizes{};
  bool has_credentials = false;
  bool has_context = false;
};

void destroy_schannel_context(SchannelTlsContext *context) {
  if (!context)
    return;

  if (context->has_context)
    DeleteSecurityContext(&context->context);
  if (context->has_credentials)
    FreeCredentialsHandle(&context->credentials);

  delete context;
}

using SchannelContextPtr =
    std::unique_ptr<SchannelTlsContext, decltype(&destroy_schannel_context)>;

SchannelTlsContext *context_from_handle(NativeTlsContextHandle handle) {
  return reinterpret_cast<SchannelTlsContext *>(handle);
}

NativeTlsContextHandle handle_from_context(SchannelTlsContext *context) {
  if (!context)
    return kInvalidNativeTlsContextHandle;
  return reinterpret_cast<NativeTlsContextHandle>(context);
}

void keep_handshake_extra(std::vector<std::uint8_t> *incoming,
                          const SecBuffer &extra_buffer) {
  if (extra_buffer.BufferType != SECBUFFER_EXTRA ||
      extra_buffer.cbBuffer == 0 ||
      extra_buffer.cbBuffer > incoming->size()) {
    incoming->clear();
    return;
  }

  const std::size_t extra_size =
      static_cast<std::size_t>(extra_buffer.cbBuffer);
  std::vector<std::uint8_t> extra(incoming->end() - extra_size,
                                  incoming->end());
  *incoming = std::move(extra);
}

bool receive_handshake_bytes(SOCKET socket,
                             std::vector<std::uint8_t> *incoming,
                             vpn_engine::ValidationResult *failure) {
  std::vector<std::uint8_t> buffer(kEncryptedReadSize);
  const int received =
      recv(socket, reinterpret_cast<char *>(buffer.data()),
           static_cast<int>(buffer.size()), 0);
  if (received == 0) {
    *failure = handshake_failed("TLS peer closed during handshake");
    return false;
  }
  if (received == SOCKET_ERROR) {
    *failure =
        handshake_failed(wsa_error_message("recv", WSAGetLastError()));
    return false;
  }

  incoming->insert(incoming->end(), buffer.begin(),
                   buffer.begin() + received);
  return true;
}

vpn_engine::ValidationResult
verify_server_certificate(SchannelTlsContext *context,
                          const std::string &sni_host) {
  PCCERT_CONTEXT certificate = nullptr;
  SECURITY_STATUS status = QueryContextAttributesA(
      &context->context, SECPKG_ATTR_REMOTE_CERT_CONTEXT, &certificate);
  if (status != SEC_E_OK || !certificate) {
    return invalid("tls_verify_failed",
                   "failed to read server certificate from Schannel context");
  }

  PCCERT_CHAIN_CONTEXT chain = nullptr;
  CERT_CHAIN_PARA chain_parameters{};
  chain_parameters.cbSize = sizeof(chain_parameters);

  const BOOL chain_ok =
      CertGetCertificateChain(nullptr, certificate, nullptr,
                              certificate->hCertStore, &chain_parameters, 0,
                              nullptr, &chain);
  if (!chain_ok || !chain) {
    CertFreeCertificateContext(certificate);
    return invalid("tls_verify_failed",
                   "failed to build server certificate chain");
  }

  std::wstring wide_sni = utf8_to_wide(sni_host);
  SSL_EXTRA_CERT_CHAIN_POLICY_PARA ssl_policy{};
  ssl_policy.cbSize = sizeof(ssl_policy);
  ssl_policy.dwAuthType = AUTHTYPE_SERVER;
  ssl_policy.fdwChecks = 0;
  ssl_policy.pwszServerName =
      wide_sni.empty() ? nullptr : const_cast<wchar_t *>(wide_sni.c_str());

  CERT_CHAIN_POLICY_PARA policy_parameters{};
  policy_parameters.cbSize = sizeof(policy_parameters);
  policy_parameters.pvExtraPolicyPara = &ssl_policy;

  CERT_CHAIN_POLICY_STATUS policy_status{};
  policy_status.cbSize = sizeof(policy_status);

  const BOOL policy_ok = CertVerifyCertificateChainPolicy(
      CERT_CHAIN_POLICY_SSL, chain, &policy_parameters, &policy_status);
  const DWORD policy_error = policy_status.dwError;

  CertFreeCertificateChain(chain);
  CertFreeCertificateContext(certificate);

  if (!policy_ok || policy_error != ERROR_SUCCESS) {
    std::ostringstream message;
    message << "server certificate verification failed with Windows error "
            << policy_error;
    return invalid("tls_verify_failed", message.str());
  }

  return {};
}

