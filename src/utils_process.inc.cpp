int run_command(const std::string &cmd) { return system(cmd.c_str()); }

std::string get_executable_path() {
#ifdef __APPLE__
  uint32_t size = 0;
  _NSGetExecutablePath(nullptr, &size);
  if (size == 0)
    return "";

  std::vector<char> buffer(size);
  if (_NSGetExecutablePath(buffer.data(), &size) != 0)
    return "";

  std::vector<char> resolved(size + 1, '\0');
  if (realpath(buffer.data(), resolved.data()))
    return resolved.data();

  return buffer.data();
#elif defined(_WIN32)
    char buf[MAX_PATH];
    DWORD len = GetModuleFileNameA(NULL, buf, MAX_PATH);
    if (len == 0 || len == MAX_PATH) return "";
    return std::string(buf);
#else
  char buf[4096];
  ssize_t len = readlink("/proc/self/exe", buf, sizeof(buf) - 1);
  if (len <= 0) return "";
  buf[len] = '\0';
  return std::string(buf);
#endif
}

std::string run_command_output(const std::string &cmd) {
  std::array<char, 256> buffer;
  std::string result;
#ifndef _WIN32
  std::unique_ptr<FILE, decltype(&pclose)> pipe(popen(cmd.c_str(), "r"),
                                                pclose);
#else
  std::unique_ptr<FILE, decltype(&_pclose)> pipe(_popen(cmd.c_str(), "r"),
                                                 _pclose);
#endif
  if (!pipe)
    return "";
  while (fgets(buffer.data(), static_cast<int>(buffer.size()), pipe.get()) !=
         nullptr) {
    result += buffer.data();
  }
  return result;
}

std::string shell_quote(const std::string &value) {
#ifndef _WIN32
  std::string quoted = "'";
  for (char c : value) {
    if (c == '\'')
      quoted += "'\\''";
    else
      quoted += c;
  }
  quoted += "'";
  return quoted;
#else
  // Windows cmd.exe: double-quote with internal escaping
  std::string quoted = "\"";
  for (char c : value) {
    if (c == '\"')
      quoted += "\\\"";
    else if (c == '%')
      quoted += "%%";
    else
      quoted += c;
  }
  quoted += "\"";
  return quoted;
#endif
}
