#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <shellapi.h>
#include <sddl.h>

#include "platform/common/backend_resolver.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/oneshot_bootstrap.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "observability/log_facade.hpp"

#include <chrono>
#include <random>
#include <sstream>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace platform {
namespace {

std::string random_hex(size_t bytes) {
  std::random_device rd;
  std::ostringstream out;
  out << std::hex;
  for (size_t i = 0; i < bytes; ++i) {
    unsigned int value = rd() & 0xffU;
    if (value < 16)
      out << '0';
    out << value;
  }
  return out.str();
}

bool wait_for_helper_pipe_available(const std::string &endpoint,
                                    DWORD *last_error) {
  for (int i = 0; i < 40; ++i) {
    if (WaitNamedPipeA(endpoint.c_str(), 100)) {
      return true;
    }
    if (last_error) {
      *last_error = GetLastError();
    }
    Sleep(100);
  }
  return false;
}

std::string current_owner_sid() {
  HANDLE token = nullptr;
  if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token))
    return "";

  DWORD needed = 0;
  GetTokenInformation(token, TokenUser, nullptr, 0, &needed);
  if (needed == 0) {
    CloseHandle(token);
    return "";
  }

  std::vector<unsigned char> buffer(needed);
  if (!GetTokenInformation(token, TokenUser, buffer.data(), needed, &needed)) {
    CloseHandle(token);
    return "";
  }
  CloseHandle(token);

  auto *user = reinterpret_cast<TOKEN_USER *>(buffer.data());
  LPSTR sid = nullptr;
  if (!ConvertSidToStringSidA(user->User.Sid, &sid))
    return "";
  std::string result = sid;
  LocalFree(sid);
  return result;
}

std::string quote_arg(const std::string &value) {
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

bool start_helper_direct(const std::string &helper_path,
                         const std::string &args, int *pid) {
  std::string cmdline = quote_arg(helper_path) + " " + args;
  std::vector<char> mutable_cmd(cmdline.begin(), cmdline.end());
  mutable_cmd.push_back('\0');

  STARTUPINFOA si = {};
  si.cb = sizeof(si);
  PROCESS_INFORMATION pi = {};
  BOOL created =
      CreateProcessA(helper_path.c_str(), mutable_cmd.data(), NULL, NULL, FALSE,
                     CREATE_NO_WINDOW, NULL, NULL, &si, &pi);
  if (!created)
    return false;

  if (pid)
    *pid = static_cast<int>(pi.dwProcessId);
  CloseHandle(pi.hThread);
  CloseHandle(pi.hProcess);
  return true;
}

} // namespace

OneshotBackend start_oneshot_helper(const OneshotBootstrapRequest &request) {
  OneshotBackend backend;
  backend.transport = "named-pipe";

  if (request.helper_path.empty()) {
    backend.code = kOneshotNotSupportedCode;
    backend.message = "exv-helper.exe path is not available.";
    return backend;
  }

  std::string session_id = random_hex(8);
  backend.endpoint = "\\\\.\\pipe\\exv-oneshot-" + session_id;
  backend.owner = current_owner_sid();
  backend.parent_pid = static_cast<int>(GetCurrentProcessId());
  if (backend.owner.empty()) {
    backend.code = kServiceStartFailedCode;
    backend.message = "Unable to determine the current Windows owner SID.";
    return backend;
  }

  std::string args = "--oneshot --endpoint \"" + backend.endpoint +
                     "\" --owner \"" + backend.owner +
                     "\" --parent-pid " + std::to_string(backend.parent_pid);

  exv::observability::LogFacade::info("Oneshot: Generated endpoint=" + backend.endpoint + " session_id=" + session_id);
  exv::observability::LogFacade::info("Oneshot: Starting helper - is_admin=" + std::string(platform::check_root() ? "true" : "false"));

  if (platform::check_root()) {
    if (!start_helper_direct(request.helper_path, args, &backend.pid)) {
      backend.code = kServiceStartFailedCode;
      backend.message = "Failed to start elevated one-shot helper.";
      return backend;
    }
  } else {
  SHELLEXECUTEINFOA sei = {};
  sei.cbSize = sizeof(sei);
  sei.fMask = SEE_MASK_NOCLOSEPROCESS;
  sei.lpVerb = "runas";
  sei.lpFile = request.helper_path.c_str();
  sei.lpParameters = args.c_str();
  sei.nShow = SW_HIDE;

  if (!ShellExecuteExA(&sei)) {
    DWORD err = GetLastError();
    backend.code = err == ERROR_CANCELLED ? kOneshotElevationDeniedCode
                                          : kServiceStartFailedCode;
    backend.message = err == ERROR_CANCELLED
                          ? "Administrator authorization was cancelled."
                          : "Failed to start elevated one-shot helper.";
    return backend;
  }

  if (sei.hProcess) {
    backend.pid = static_cast<int>(GetProcessId(sei.hProcess));
    CloseHandle(sei.hProcess);
  }
  }

  DWORD wait_error = ERROR_SUCCESS;
  if (!wait_for_helper_pipe_available(backend.endpoint, &wait_error)) {
    exv::observability::LogFacade::error(
        "Oneshot: Helper pipe did not become available - endpoint=" +
        backend.endpoint + " last_error=" + std::to_string(wait_error));
    backend.code = kHelperRpcFailedCode;
    backend.message = "One-shot helper did not become ready.";
    return backend;
  }

  exv::observability::LogFacade::info("Oneshot: Helper started successfully - endpoint=" + backend.endpoint + " pid=" + std::to_string(backend.pid));
  backend.ok = true;
  return backend;
}

} // namespace platform
} // namespace ecnuvpn
