#include "platform/common/openconnect_process.hpp"

#include "logger.hpp"
#include "utils.hpp"

#include <filesystem>
#include <functional>
#include <sstream>
#include <vector>
#include <windows.h>

namespace ecnuvpn {
namespace platform {
namespace {

std::string windows_generated_interface_name(const Config &cfg) {
  std::ostringstream seed;
  seed << cfg.username << "@" << cfg.server;
  std::size_t hash = std::hash<std::string>{}(seed.str());
  std::ostringstream name;
  name << "ECNUVPN-" << std::uppercase << std::hex
       << static_cast<unsigned long long>(hash & 0xffffffffULL);
  return name.str();
}

std::string select_windows_interface_name(const Config &cfg) {
  if (cfg.windows_tunnel_driver == "tap")
    return cfg.windows_tap_interface;

  if (cfg.windows_tunnel_driver == "wintun")
    return windows_generated_interface_name(cfg);

  if (!utils::get_bundled_wintun_path().empty())
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
  cmd << windows_quote_arg(utils::get_executable_path()) << " __tunnel-script";
  return cmd.str();
}

std::string build_openconnect_command_line(const Config &cfg) {
  std::vector<std::string> args;
  std::string openconnect_path = utils::get_openconnect_path(cfg.openconnect_runtime);
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
  HANDLE handle = CreateFileA(path.c_str(), FILE_APPEND_DATA | GENERIC_WRITE,
                              FILE_SHARE_READ | FILE_SHARE_WRITE, &sa,
                              OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
  if (handle != INVALID_HANDLE_VALUE)
    SetFilePointer(handle, 0, NULL, FILE_END);
  return handle;
}

} // namespace

bool spawn_openconnect_process(const Config &cfg, const std::string &password,
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

  std::string log_path = utils::expand_home(cfg.log_file);
  HANDLE log_handle = open_inheritable_append_handle(log_path);
  if (log_handle == INVALID_HANDLE_VALUE) {
    CloseHandle(stdin_read);
    CloseHandle(stdin_write);
    logger::error("Failed to open VPN log file for Windows launch: " + log_path);
    return false;
  }

  std::string openconnect_path = utils::get_openconnect_path(cfg.openconnect_runtime);
  if (openconnect_path.empty()) {
    CloseHandle(stdin_read);
    CloseHandle(stdin_write);
    CloseHandle(log_handle);
    logger::error("Bundled/system openconnect binary could not be resolved.");
    return false;
  }

  std::string cmdline = build_openconnect_command_line(cfg);
  std::vector<char> mutable_cmd(cmdline.begin(), cmdline.end());
  mutable_cmd.push_back('\0');
  std::string current_dir =
      std::filesystem::path(openconnect_path).parent_path().string();

  STARTUPINFOA si = {};
  si.cb = sizeof(si);
  si.dwFlags = STARTF_USESTDHANDLES;
  si.hStdInput = stdin_read;
  si.hStdOutput = log_handle;
  si.hStdError = log_handle;
  PROCESS_INFORMATION pi = {};
  BOOL created = CreateProcessA(openconnect_path.c_str(), mutable_cmd.data(),
                                NULL, NULL, TRUE, CREATE_NO_WINDOW, NULL,
                                current_dir.empty() ? NULL : current_dir.c_str(),
                                &si, &pi);
  CloseHandle(stdin_read);
  CloseHandle(log_handle);
  if (!created) {
    CloseHandle(stdin_write);
    logger::error("Failed to create openconnect process: " +
                  std::to_string(GetLastError()));
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
