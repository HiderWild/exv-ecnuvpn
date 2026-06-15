#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/win32/windows_strings.hpp"
#include "platform/common/openconnect_process.hpp"

#include "common/diagnostics/logger.hpp"

#include <filesystem>
#include <functional>
#include <sstream>
#include <vector>
#include <windows.h>

namespace ecnuvpn {
namespace platform {
namespace {

std::string windows_generated_interface_name(const ConfigView &cfg) {
  std::ostringstream seed;
  seed << cfg.username << "@" << cfg.server;
  std::size_t hash = std::hash<std::string>{}(seed.str());
  std::ostringstream name;
  name << "ECNUVPN-" << std::uppercase << std::hex
       << static_cast<unsigned long long>(hash & 0xffffffffULL);
  return name.str();
}

std::string select_windows_interface_name(const ConfigView &cfg) {
  if (cfg.windows_tunnel_driver == "tap")
    return cfg.windows_tap_interface;

  if (cfg.windows_tunnel_driver == "wintun")
    return windows_generated_interface_name(cfg);

  if (!platform::get_bundled_wintun_path().empty())
    return windows_generated_interface_name(cfg);

  return cfg.windows_tap_interface;
}

std::string windows_quote_arg(const std::string &value) {
  if (value.empty())
    return "\"\"";
  bool needs_quotes = value.find_first_of(" \t\"") != std::string::npos;
  if (!needs_quotes)
    return value;

  std::string quoted = "\"";
  unsigned int backslashes = 0;
  for (char c : value) {
    if (c == '\\') {
      ++backslashes;
      continue;
    }
    if (c == '"') {
      quoted.append(backslashes * 2 + 1, '\\');
      quoted.push_back('"');
      backslashes = 0;
      continue;
    }
    quoted.append(backslashes, '\\');
    backslashes = 0;
    quoted.push_back(c);
  }
  quoted.append(backslashes * 2, '\\');
  quoted.push_back('"');
  return quoted;
}

std::string windows_openconnect_script_command() {
  std::ostringstream cmd;
  cmd << windows_quote_arg(platform::get_executable_path()) << " __tunnel-script";
  return cmd.str();
}

std::string build_openconnect_command_line(const ConfigView &cfg) {
  std::vector<std::string> args;
  std::string openconnect_path = platform::get_openconnect_path(cfg.openconnect_runtime);
  args.push_back(openconnect_path.empty() ? std::string("openconnect.exe")
                                          : openconnect_path);
  args.push_back(cfg.server);
  args.push_back("--useragent");
  args.push_back(cfg.useragent);
  args.push_back("-m");
  args.push_back(std::to_string(cfg.mtu));
  if (cfg.mtu > 0) {
    args.push_back("--base-mtu");
    args.push_back(std::to_string(cfg.mtu));
  }
  args.push_back("-u");
  args.push_back(cfg.username);
  args.push_back("--passwd-on-stdin");
  args.push_back("--non-inter");
  args.push_back("--script");
  args.push_back(windows_openconnect_script_command());
  std::string interface_name = select_windows_interface_name(cfg);
  if (!interface_name.empty()) {
    args.push_back("--interface");
    args.push_back(interface_name);
  }
  if (cfg.disable_dtls) {
    args.push_back("--no-dtls");
  }
  for (const auto &arg : cfg.extra_args) {
    args.push_back(arg);
  }

  std::ostringstream cmd;
  for (std::size_t i = 0; i < args.size(); ++i) {
    if (i)
      cmd << ' ';
    cmd << windows_quote_arg(args[i]);
  }
  return cmd.str();
}

HANDLE open_inheritable_append_handle(const std::string &path) {
  std::error_code ec;
  std::filesystem::path parent = std::filesystem::path(path).parent_path();
  if (!parent.empty())
    std::filesystem::create_directories(parent, ec);

  SECURITY_ATTRIBUTES sa = {};
  sa.nLength = sizeof(sa);
  sa.bInheritHandle = TRUE;
  std::wstring wide_path = platform::wide_from_utf8(path);
  HANDLE handle = CreateFileW(wide_path.empty() ? L"" : wide_path.c_str(),
                              FILE_APPEND_DATA | GENERIC_WRITE,
                              FILE_SHARE_READ | FILE_SHARE_WRITE, &sa,
                              OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
  if (handle != INVALID_HANDLE_VALUE)
    SetFilePointer(handle, 0, NULL, FILE_END);
  return handle;
}

void set_child_environment_override(const char *name, const std::string &value) {
  std::wstring wide_name = platform::wide_from_utf8(name);
  std::wstring wide_value = platform::wide_from_utf8(value);
  if (!wide_name.empty())
    SetEnvironmentVariableW(wide_name.c_str(),
                            wide_value.empty() ? nullptr : wide_value.c_str());
}

} // namespace

bool spawn_openconnect_process(const ConfigView &cfg, const std::string &password,
                               OpenconnectProcess *process) {
  if (process) {
    process->pid = -1;
    process->wait_handle = nullptr;
  }

  SECURITY_ATTRIBUTES sa = {};
  sa.nLength = sizeof(sa);
  sa.bInheritHandle = TRUE;
  HANDLE stdin_read = NULL;
  HANDLE stdin_write = NULL;
  if (!CreatePipe(&stdin_read, &stdin_write, &sa, 0)) {
    logger::error("Failed to create password pipe for openconnect.");
    return false;
  }
  SetHandleInformation(stdin_write, HANDLE_FLAG_INHERIT, 0);

  std::string log_path = platform::expand_home(cfg.log_file);
  HANDLE log_handle = open_inheritable_append_handle(log_path);
  if (log_handle == INVALID_HANDLE_VALUE) {
    CloseHandle(stdin_read);
    CloseHandle(stdin_write);
    logger::error("Failed to open VPN log file for Windows launch: " + log_path);
    return false;
  }

  std::string openconnect_path = platform::get_openconnect_path(cfg.openconnect_runtime);
  if (openconnect_path.empty()) {
    CloseHandle(stdin_read);
    CloseHandle(stdin_write);
    CloseHandle(log_handle);
    logger::error("Bundled/system openconnect binary could not be resolved.");
    return false;
  }

  std::string cmdline = build_openconnect_command_line(cfg);
  std::wstring wide_cmdline = platform::wide_from_utf8(cmdline);
  std::string current_dir =
      std::filesystem::path(openconnect_path).parent_path().string();
  std::wstring wide_openconnect_path = platform::wide_from_utf8(openconnect_path);
  std::wstring wide_current_dir = platform::wide_from_utf8(current_dir);

  set_child_environment_override("ECNUVPN_HOME", platform::get_effective_home());
  set_child_environment_override("ECNUVPN_CONFIG_DIR", platform::get_config_dir());
  set_child_environment_override("LANG", "C.UTF-8");
  set_child_environment_override("LC_ALL", "C.UTF-8");

  STARTUPINFOW si = {};
  si.cb = sizeof(si);
  si.dwFlags = STARTF_USESTDHANDLES;
  si.hStdInput = stdin_read;
  si.hStdOutput = log_handle;
  si.hStdError = log_handle;
  PROCESS_INFORMATION pi = {};
  BOOL created = CreateProcessW(wide_openconnect_path.c_str(),
                                wide_cmdline.empty() ? nullptr : wide_cmdline.data(),
                                NULL, NULL, TRUE, CREATE_NO_WINDOW, NULL,
                                wide_current_dir.empty() ? NULL : wide_current_dir.c_str(),
                                &si, &pi);
  CloseHandle(stdin_read);
  CloseHandle(log_handle);
  if (!created) {
    DWORD error = GetLastError();
    CloseHandle(stdin_write);
    logger::error("Failed to create openconnect process: " +
                  platform::windows_error_message(error));
    return false;
  }

  std::string stdin_payload = password;
  stdin_payload.push_back('\n');
  DWORD written = 0;
  BOOL wrote = WriteFile(stdin_write, stdin_payload.data(),
                         static_cast<DWORD>(stdin_payload.size()), &written,
                         NULL);
  CloseHandle(pi.hThread);
  if (!wrote) {
    CloseHandle(stdin_write);
    TerminateProcess(pi.hProcess, 1);
    CloseHandle(pi.hProcess);
    logger::error("Failed to write password to openconnect stdin.");
    return false;
  }

  if (process) {
    process->pid = static_cast<int>(pi.dwProcessId);
    process->wait_handle = pi.hProcess;
  } else {
    CloseHandle(pi.hProcess);
  }
  return true;
}

void close_openconnect_process(OpenconnectProcess *process) {
  if (!process || !process->wait_handle)
    return;
  CloseHandle(static_cast<HANDLE>(process->wait_handle));
  process->wait_handle = nullptr;
}

} // namespace platform
} // namespace ecnuvpn
