#include "platform/common/helper_lifecycle.hpp"

#include "helper_ipc.hpp"
#include "logger.hpp"
#include "utils.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <string>
#include <thread>
#include <vector>

namespace ecnuvpn {
namespace platform {

void cleanup_routes() {}

void kill_all_supervisors() {}

void fix_config_dir_ownership() {}

int copy_self_to_stable_path_and_reexec(const std::string &) {
  return 1;
}

bool is_process_alive(int pid) {
  if (pid <= 0)
    return false;
  HANDLE hProcess = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, static_cast<DWORD>(pid));
  if (!hProcess)
    return false;
  DWORD exitCode = 0;
  BOOL ok = GetExitCodeProcess(hProcess, &exitCode);
  CloseHandle(hProcess);
  return ok && exitCode == STILL_ACTIVE;
}

int find_openconnect_pid() {
  std::string output = utils::run_command_output("tasklist /FI \"IMAGENAME eq openconnect.exe\" /NH /FO CSV 2>nul");
  auto pos = output.find(',');
  if (pos == std::string::npos || pos < 2)
    return -1;
  auto start = output.find('"', pos + 1);
  if (start == std::string::npos)
    return -1;
  auto end = output.find('"', start + 1);
  if (end == std::string::npos)
    return -1;
  std::string pid_str = output.substr(start + 1, end - start - 1);
  try {
    return std::stoi(pid_str);
  } catch (...) {
    return -1;
  }
}

std::string get_interfaces_output() {
  return utils::run_command_output("netsh interface show interface 2>nul");
}

std::string create_temp_request_file(const std::string &payload) {
  char temp_path[MAX_PATH];
  if (GetTempPathA(MAX_PATH, temp_path) == 0)
    return "";

  char temp_file[MAX_PATH];
  if (GetTempFileNameA(temp_path, "exv", 0, temp_file) == 0)
    return "";

  HANDLE hFile = CreateFileA(temp_file, GENERIC_WRITE, 0, NULL,
                             CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
  if (hFile == INVALID_HANDLE_VALUE) {
    DeleteFileA(temp_file);
    return "";
  }

  DWORD written = 0;
  BOOL ok = WriteFile(hFile, payload.data(), static_cast<DWORD>(payload.size()),
                      &written, NULL);
  CloseHandle(hFile);

  if (!ok || written != payload.size()) {
    DeleteFileA(temp_file);
    return "";
  }

  return temp_file;
}

int spawn_worker_process(const std::string &executable_path,
                         const std::string &request_path) {
  std::string cmdline =
      "\"" + executable_path + "\" __helper-exec \"" + request_path + "\"";
  std::vector<char> mutable_cmd(cmdline.begin(), cmdline.end());
  mutable_cmd.push_back('\0');
  STARTUPINFOA si = {};
  si.cb = sizeof(si);
  PROCESS_INFORMATION pi = {};
  if (!CreateProcessA(executable_path.c_str(), mutable_cmd.data(), NULL, NULL, TRUE,
                       CREATE_NO_WINDOW, NULL, NULL, &si, &pi)) {
    return -1;
  }
  CloseHandle(pi.hThread);
  WaitForSingleObject(pi.hProcess, INFINITE);
  DWORD exitCode = 1;
  GetExitCodeProcess(pi.hProcess, &exitCode);
  CloseHandle(pi.hProcess);
  return (exitCode == 0) ? 0 : 1;
}

void terminate_process(int pid) {
  if (pid <= 0)
    return;
  HANDLE h = OpenProcess(PROCESS_TERMINATE, FALSE, static_cast<DWORD>(pid));
  if (h) {
    TerminateProcess(h, 1);
    CloseHandle(h);
  }
}

void sleep_ms(int milliseconds) {
  Sleep(static_cast<DWORD>(milliseconds));
}

void reap_children() {
  // No-op on Windows; child process reaping is not needed.
}

void dispatch_request_background(
    helper::IpcServer &ipc, const std::string &raw_request,
    unsigned int peer_uid, unsigned int peer_gid,
    std::function<nlohmann::json(unsigned int, unsigned int,
                                  const nlohmann::json &)> handler) {
  // Windows: use threads instead of fork (no fork available)
  // Each request is processed in a background thread so long-running
  // operations (e.g. VPN start) don't block new client connections.
  helper::IpcServer *ipc_ptr = &ipc;
  std::thread([ipc_ptr, raw_request, peer_uid, peer_gid, handler]() {
    nlohmann::json response;
    try {
      nlohmann::json request = nlohmann::json::parse(raw_request);
      response = handler(peer_uid, peer_gid, request);
    } catch (...) {
      response = nlohmann::json{{"ok", false}, {"message", "Failed to parse helper request."}};
    }
    ipc_ptr->send_response(response.dump());
  }).detach();
}

} // namespace platform
} // namespace ecnuvpn
