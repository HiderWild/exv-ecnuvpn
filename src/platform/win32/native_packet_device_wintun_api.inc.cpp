struct PacketWintunApi {
  WintunOpenAdapterFn open_adapter = nullptr;
  WintunCreateAdapterFn create_adapter = nullptr;
  WintunCloseAdapterFn close_adapter = nullptr;
  WintunGetAdapterLuidFn get_adapter_luid = nullptr;
  WintunStartSessionFn start_session = nullptr;
  WintunEndSessionFn end_session = nullptr;
  WintunReceivePacketFn receive_packet = nullptr;
  WintunReleaseReceivePacketFn release_receive_packet = nullptr;
  WintunAllocateSendPacketFn allocate_send_packet = nullptr;
  WintunSendPacketFn send_packet = nullptr;
};

vpn_engine::ValidationResult invalid(std::string code, std::string message) {
  vpn_engine::ValidationResult result;
  result.ok = false;
  result.code = std::move(code);
  result.message = std::move(message);
  return result;
}

NativeWintunStartResult wintun_failure(NativeWintunError error,
                                       std::string detail = {}) {
  NativeWintunStartResult result;
  result.error = error;
  result.detail = std::move(detail);
  return result;
}

std::wstring widen_utf8(const std::string &value) {
  if (value.empty())
    return {};

  int size = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.c_str(),
                                 static_cast<int>(value.size()), nullptr, 0);
  if (size > 0) {
    std::wstring result(static_cast<std::size_t>(size), L'\0');
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.c_str(),
                        static_cast<int>(value.size()), &result[0], size);
    return result;
  }

  std::wstring fallback;
  fallback.reserve(value.size());
  for (unsigned char ch : value)
    fallback.push_back(static_cast<wchar_t>(ch));
  return fallback;
}

std::string narrow_utf8(const std::wstring &value) {
  if (value.empty())
    return {};

  int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value.c_str(),
                                 static_cast<int>(value.size()), nullptr, 0,
                                 nullptr, nullptr);
  if (size > 0) {
    std::string result(static_cast<std::size_t>(size), '\0');
    WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value.c_str(),
                        static_cast<int>(value.size()), &result[0], size,
                        nullptr, nullptr);
    return result;
  }

  std::string fallback;
  fallback.reserve(value.size());
  for (wchar_t ch : value)
    fallback.push_back(ch >= 0 && ch <= 0x7f ? static_cast<char>(ch) : '?');
  return fallback;
}

bool file_exists(const std::wstring &path) {
  if (path.empty())
    return false;

  DWORD attributes = GetFileAttributesW(path.c_str());
  return attributes != INVALID_FILE_ATTRIBUTES &&
         (attributes & FILE_ATTRIBUTE_DIRECTORY) == 0;
}

template <typename Fn>
bool load_proc(HMODULE module, const char *name, Fn &target,
               std::string *error) {
  FARPROC proc = GetProcAddress(module, name);
  if (!proc) {
    if (error)
      *error = std::string("missing Wintun export: ") + name;
    return false;
  }
#if defined(__GNUC__)
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wcast-function-type"
#endif
  target = reinterpret_cast<Fn>(proc);
#if defined(__GNUC__)
#pragma GCC diagnostic pop
#endif
  return true;
}

bool load_packet_wintun_api(HMODULE module, PacketWintunApi *api,
                            std::string *error) {
  if (!module || !api)
    return false;

  return load_proc(module, "WintunOpenAdapter", api->open_adapter, error) &&
         load_proc(module, "WintunCreateAdapter", api->create_adapter,
                   error) &&
         load_proc(module, "WintunCloseAdapter", api->close_adapter, error) &&
         load_proc(module, "WintunGetAdapterLUID", api->get_adapter_luid,
                   error) &&
         load_proc(module, "WintunStartSession", api->start_session, error) &&
         load_proc(module, "WintunEndSession", api->end_session, error) &&
         load_proc(module, "WintunReceivePacket", api->receive_packet,
                   error) &&
         load_proc(module, "WintunReleaseReceivePacket",
                   api->release_receive_packet, error) &&
         load_proc(module, "WintunAllocateSendPacket",
                   api->allocate_send_packet, error) &&
         load_proc(module, "WintunSendPacket", api->send_packet, error);
}

std::string packet_error_message(const char *operation, DWORD system_error) {
  return std::string(operation) + " failed with Windows error " +
         std::to_string(static_cast<unsigned long>(system_error));
}

