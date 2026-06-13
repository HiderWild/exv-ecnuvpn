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

