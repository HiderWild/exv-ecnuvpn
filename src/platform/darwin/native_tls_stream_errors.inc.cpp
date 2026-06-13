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

