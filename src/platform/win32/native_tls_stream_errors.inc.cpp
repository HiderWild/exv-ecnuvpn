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

std::string security_status_hex(SECURITY_STATUS status) {
  std::ostringstream out;
  out << "0x" << std::hex << static_cast<unsigned long>(status);
  return out.str();
}

std::string wsa_error_message(const char *operation, int error) {
  std::ostringstream out;
  out << operation << " failed with WSA error " << error;
  return out.str();
}

bool valid_socket_handle(NativeTlsSocketHandle socket) {
  return socket != kInvalidNativeTlsSocketHandle;
}

bool valid_context_handle(NativeTlsContextHandle context) {
  return context != kInvalidNativeTlsContextHandle;
}

SOCKET socket_from_handle(NativeTlsSocketHandle socket) {
  return static_cast<SOCKET>(socket);
}

NativeTlsSocketHandle handle_from_socket(SOCKET socket) {
  if (socket == INVALID_SOCKET)
    return kInvalidNativeTlsSocketHandle;
  return static_cast<NativeTlsSocketHandle>(socket);
}

std::wstring utf8_to_wide(const std::string &input) {
  if (input.empty())
    return {};

  int chars = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, input.data(),
                                  static_cast<int>(input.size()), nullptr, 0);
  if (chars <= 0) {
    chars = MultiByteToWideChar(CP_ACP, 0, input.data(),
                                static_cast<int>(input.size()), nullptr, 0);
  }
  if (chars <= 0)
    return {};

  std::wstring output(static_cast<std::size_t>(chars), L'\0');
  const UINT code_page = MultiByteToWideChar(
                             CP_UTF8, MB_ERR_INVALID_CHARS, input.data(),
                             static_cast<int>(input.size()), nullptr, 0) > 0
                             ? CP_UTF8
                             : CP_ACP;
  const DWORD flags = code_page == CP_UTF8 ? MB_ERR_INVALID_CHARS : 0;
  MultiByteToWideChar(code_page, flags, input.data(),
                      static_cast<int>(input.size()), &output[0], chars);
  return output;
}

