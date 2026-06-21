#include "platform/common/process_utils.hpp"

#include <array>
#include <cstdio>
#include <cstdlib>
#include <memory>
#include <string>
#include <vector>
#ifdef __APPLE__
#include <mach-o/dyld.h>
#endif
#ifdef _WIN32
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#else
#include <unistd.h>
#endif

namespace exv::platform {
namespace {

std::string base64_encode(const std::vector<unsigned char> &bytes) {
  static constexpr char kTable[] =
      "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  std::string encoded;
  encoded.reserve(((bytes.size() + 2) / 3) * 4);

  for (std::size_t i = 0; i < bytes.size(); i += 3) {
    const unsigned int b0 = bytes[i];
    const unsigned int b1 = i + 1 < bytes.size() ? bytes[i + 1] : 0;
    const unsigned int b2 = i + 2 < bytes.size() ? bytes[i + 2] : 0;
    encoded.push_back(kTable[(b0 >> 2) & 0x3f]);
    encoded.push_back(kTable[((b0 & 0x03) << 4) | ((b1 >> 4) & 0x0f)]);
    encoded.push_back(i + 1 < bytes.size()
                          ? kTable[((b1 & 0x0f) << 2) | ((b2 >> 6) & 0x03)]
                          : '=');
    encoded.push_back(i + 2 < bytes.size() ? kTable[b2 & 0x3f] : '=');
  }
  return encoded;
}

std::vector<unsigned char> utf16le_bytes(const std::string &text) {
  std::vector<unsigned char> bytes;
#ifdef _WIN32
  const int wide_length =
      MultiByteToWideChar(CP_UTF8, 0, text.data(),
                          static_cast<int>(text.size()), nullptr, 0);
  if (wide_length > 0) {
    std::wstring wide(static_cast<std::size_t>(wide_length), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()),
                        wide.data(), wide_length);
    bytes.reserve(wide.size() * 2);
    for (wchar_t ch : wide) {
      bytes.push_back(static_cast<unsigned char>(ch & 0xff));
      bytes.push_back(static_cast<unsigned char>((ch >> 8) & 0xff));
    }
    return bytes;
  }
#endif
  bytes.reserve(text.size() * 2);
  for (unsigned char ch : text) {
    bytes.push_back(ch);
    bytes.push_back(0);
  }
  return bytes;
}

} // namespace

int run_command(const std::string &cmd) { return std::system(cmd.c_str()); }

std::string get_executable_path() {
#ifdef __APPLE__
  uint32_t size = 0;
  _NSGetExecutablePath(nullptr, &size);
  if (size == 0) {
    return "";
  }

  std::vector<char> buffer(size);
  if (_NSGetExecutablePath(buffer.data(), &size) != 0) {
    return "";
  }

  std::vector<char> resolved(size + 1, '\0');
  if (realpath(buffer.data(), resolved.data())) {
    return resolved.data();
  }

  return buffer.data();
#elif defined(_WIN32)
  char buf[MAX_PATH];
  DWORD len = GetModuleFileNameA(NULL, buf, MAX_PATH);
  if (len == 0 || len == MAX_PATH) {
    return "";
  }
  return std::string(buf);
#else
  char buf[4096];
  ssize_t len = readlink("/proc/self/exe", buf, sizeof(buf) - 1);
  if (len <= 0) {
    return "";
  }
  buf[len] = '\0';
  return std::string(buf);
#endif
}

std::string powershell_command() {
#ifdef _WIN32
  char windows_dir[MAX_PATH] = {};
  const UINT length = GetSystemWindowsDirectoryA(windows_dir, MAX_PATH);
  if (length > 0 && length < MAX_PATH) {
    return std::string(windows_dir) +
           "\\System32\\WindowsPowerShell\\v1.0\\powershell.exe";
  }
#endif
  return "powershell.exe";
}

std::string powershell_encoded_command(const std::string &script,
                                       const std::string &arguments) {
  std::string command = powershell_command();
  if (!arguments.empty()) {
    command += " " + arguments;
  }
  std::string wrapped_script =
      "$ProgressPreference='SilentlyContinue';" + script;
  command +=
      " -EncodedCommand " + base64_encode(utf16le_bytes(wrapped_script));
  return command;
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
  if (!pipe) {
    return "";
  }
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
    if (c == '\'') {
      quoted += "'\\''";
    } else {
      quoted += c;
    }
  }
  quoted += "'";
  return quoted;
#else
  std::string quoted = "\"";
  for (char c : value) {
    if (c == '\"') {
      quoted += "\\\"";
    } else if (c == '%') {
      quoted += "%%";
    } else {
      quoted += c;
    }
  }
  quoted += "\"";
  return quoted;
#endif
}

bool check_root() {
#ifndef _WIN32
  return geteuid() == 0;
#else
  SID_IDENTIFIER_AUTHORITY nt_auth = SECURITY_NT_AUTHORITY;
  PSID admin_group = nullptr;
  BOOL is_admin = FALSE;
  if (AllocateAndInitializeSid(&nt_auth, 2, SECURITY_BUILTIN_DOMAIN_RID,
                               DOMAIN_ALIAS_RID_ADMINS, 0, 0, 0, 0, 0, 0,
                               &admin_group)) {
    CheckTokenMembership(nullptr, admin_group, &is_admin);
    FreeSid(admin_group);
  }
  return is_admin ? true : false;
#endif
}

} // namespace exv::platform

