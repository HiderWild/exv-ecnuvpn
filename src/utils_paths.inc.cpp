std::string get_home_for_uid(uid_t uid) {
  return platform::home_for_uid(static_cast<unsigned int>(uid));
}

std::string get_username_for_uid(uid_t uid) {
  return platform::username_for_uid(static_cast<unsigned int>(uid));
}

std::string get_effective_home() {
  if (!runtime_home_override.empty())
    return runtime_home_override;

  return platform::effective_home();
}

std::string expand_home(const std::string &path) {
  return expand_home_with_base(path, get_effective_home());
}

std::string get_redirect_path() {
  return platform::redirect_path_for_home(get_effective_home());
}

std::string get_config_dir() {
  if (!runtime_config_dir_override.empty())
    return runtime_config_dir_override;

  return get_config_dir_for_home(get_effective_home());
}

std::string get_config_dir_for_uid(uid_t uid) {
  return get_config_dir_for_home(get_home_for_uid(uid));
}

void set_runtime_path_override(const std::string &home,
                               const std::string &config_dir) {
  runtime_home_override = home;
  runtime_config_dir_override = config_dir.empty()
                                    ? get_config_dir_for_home(home)
                                    : expand_home_with_base(config_dir, home);
}

void clear_runtime_path_override() {
  runtime_home_override.clear();
  runtime_config_dir_override.clear();
}

void set_runtime_owner(uid_t uid, gid_t gid) {
  runtime_owner_uid = uid;
  runtime_owner_gid = gid;
}

void clear_runtime_owner() {
  runtime_owner_uid = static_cast<uid_t>(-1);
  runtime_owner_gid = static_cast<gid_t>(-1);
}

bool has_runtime_owner() {
  return runtime_owner_uid != static_cast<uid_t>(-1) &&
         runtime_owner_gid != static_cast<gid_t>(-1);
}

uid_t get_runtime_owner_uid() { return runtime_owner_uid; }

gid_t get_runtime_owner_gid() { return runtime_owner_gid; }

bool sync_owner(const std::string &path) {
  if (!has_runtime_owner())
    return true;
  if (!file_exists(path))
    return false;
  return platform::sync_owner(path, static_cast<unsigned int>(runtime_owner_uid),
                              static_cast<unsigned int>(runtime_owner_gid));
}

bool set_config_dir(const std::string &dir) {
  std::string expanded = expand_home(dir);
  if (!ensure_dir(expanded))
    return false;
  // Write redirect file
  std::ofstream wf(get_redirect_path());
  if (!wf.is_open())
    return false;
  wf << dir; // store as-is (may contain ~)
  return wf.good() && sync_owner(get_redirect_path());
}

std::string get_config_path() {
  return platform::config_path(get_config_dir());
}

#ifdef _WIN32
std::wstring wide_from_utf8(const std::string &value) {
  if (value.empty())
    return std::wstring();
  int length = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.c_str(),
                                   static_cast<int>(value.size()), nullptr, 0);
  if (length <= 0) {
    length = MultiByteToWideChar(CP_ACP, 0, value.c_str(),
                                 static_cast<int>(value.size()), nullptr, 0);
  }
  if (length <= 0)
    return std::wstring();

  std::wstring result(static_cast<std::size_t>(length), L'\0');
  UINT codepage = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS,
                                      value.c_str(),
                                      static_cast<int>(value.size()), nullptr,
                                      0) > 0
                      ? CP_UTF8
                      : CP_ACP;
  MultiByteToWideChar(codepage, codepage == CP_UTF8 ? MB_ERR_INVALID_CHARS : 0,
                      value.c_str(), static_cast<int>(value.size()),
                      result.data(), length);
  return result;
}

std::string utf8_from_wide(const std::wstring &value) {
  if (value.empty())
    return std::string();
  int length = WideCharToMultiByte(CP_UTF8, 0, value.c_str(),
                                   static_cast<int>(value.size()), nullptr, 0,
                                   nullptr, nullptr);
  if (length <= 0)
    return std::string();

  std::string result(static_cast<std::size_t>(length), '\0');
  WideCharToMultiByte(CP_UTF8, 0, value.c_str(),
                      static_cast<int>(value.size()), result.data(), length,
                      nullptr, nullptr);
  return result;
}

std::string windows_error_message(unsigned long error_code) {
  wchar_t *buffer = nullptr;
  DWORD flags = FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM |
                FORMAT_MESSAGE_IGNORE_INSERTS;
  DWORD length = FormatMessageW(flags, nullptr, static_cast<DWORD>(error_code),
                                MAKELANGID(LANG_NEUTRAL, SUBLANG_DEFAULT),
                                reinterpret_cast<LPWSTR>(&buffer), 0, nullptr);
  if (length == 0 || !buffer)
    return std::to_string(error_code);

  std::wstring message(buffer, length);
  LocalFree(buffer);
  while (!message.empty() &&
         (message.back() == L'\r' || message.back() == L'\n' ||
          message.back() == L'.' || message.back() == L' ')) {
    message.pop_back();
  }
  return utf8_from_wide(message) + " (" + std::to_string(error_code) + ")";
}
#endif
