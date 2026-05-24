#include "platform/common/vpn_supervisor_process.hpp"

#include "logger.hpp"
#include "utils.hpp"

#include <nlohmann/json.hpp>

#include <string>
#include <vector>
#include <windows.h>

namespace ecnuvpn {
namespace platform {
namespace {

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

HANDLE open_inheritable_null_handle(DWORD access) {
  SECURITY_ATTRIBUTES sa = {};
  sa.nLength = sizeof(sa);
  sa.bInheritHandle = TRUE;
  return CreateFileA("NUL", access, FILE_SHARE_READ | FILE_SHARE_WRITE, &sa,
                     OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
}

} // namespace

bool spawn_vpn_supervisor_process(const Config &cfg,
                                  const std::string &password,
                                  int retry_limit,
                                  SupervisorEntryPoint entry_point,
                                  int *pid) {
  (void)entry_point;
  if (pid)
    *pid = -1;

  nlohmann::json request{{"config", cfg},
                         {"password", password},
                         {"retry_limit", retry_limit},
                         {"home", utils::get_effective_home()},
                         {"config_dir", utils::get_config_dir()}};
  std::string payload = request.dump();

  SECURITY_ATTRIBUTES sa = {};
  sa.nLength = sizeof(sa);
  sa.bInheritHandle = TRUE;
  HANDLE stdin_read = NULL;
  HANDLE stdin_write = NULL;
  if (!CreatePipe(&stdin_read, &stdin_write, &sa, 0)) {
    logger::error("Failed to create Windows supervisor pipe.");
    return false;
  }
  SetHandleInformation(stdin_write, HANDLE_FLAG_INHERIT, 0);

  HANDLE null_handle = open_inheritable_null_handle(GENERIC_WRITE);
  if (null_handle == INVALID_HANDLE_VALUE) {
    CloseHandle(stdin_read);
    CloseHandle(stdin_write);
    logger::error("Failed to open NUL handle for Windows supervisor.");
    return false;
  }

  std::string exec_path = utils::get_executable_path();
  std::string cmdline = windows_quote_arg(exec_path) + " __vpn-supervisor";
  std::vector<char> mutable_cmd(cmdline.begin(), cmdline.end());
  mutable_cmd.push_back('\0');

  STARTUPINFOA si = {};
  si.cb = sizeof(si);
  si.dwFlags = STARTF_USESTDHANDLES;
  si.hStdInput = stdin_read;
  si.hStdOutput = null_handle;
  si.hStdError = null_handle;
  PROCESS_INFORMATION pi = {};
  BOOL created = CreateProcessA(exec_path.c_str(), mutable_cmd.data(), NULL,
                                NULL, TRUE, CREATE_NO_WINDOW, NULL, NULL,
                                &si, &pi);
  CloseHandle(stdin_read);
  CloseHandle(null_handle);
  if (!created) {
    CloseHandle(stdin_write);
    logger::error("Failed to create reconnect supervisor process: " +
                  std::to_string(GetLastError()));
    return false;
  }

  DWORD written = 0;
  BOOL wrote = WriteFile(stdin_write, payload.data(),
                         static_cast<DWORD>(payload.size()), &written, NULL);
  CloseHandle(stdin_write);
  CloseHandle(pi.hThread);
  if (!wrote) {
    TerminateProcess(pi.hProcess, 1);
    CloseHandle(pi.hProcess);
    logger::error("Failed to send startup payload to Windows reconnect supervisor.");
    return false;
  }

  if (pid)
    *pid = static_cast<int>(pi.dwProcessId);
  CloseHandle(pi.hProcess);
  return true;
}

} // namespace platform
} // namespace ecnuvpn