#include "helper/platform/helper_lifecycle.hpp"

#include "helper/helper_ipc.hpp"
#include "logger.hpp"
#include "utils.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <string>
#include <vector>

namespace ecnuvpn {
namespace platform {

void cleanup_routes() {}

void kill_all_supervisors() {}

void fix_config_dir_ownership() {}

int copy_self_to_stable_path_and_reexec(const std::string &) {
  return 1;
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
  return static_cast<int>(exitCode);
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

void force_terminate_process(int pid) { terminate_process(pid); }

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
  nlohmann::json response;
  try {
    nlohmann::json request = nlohmann::json::parse(raw_request);
    response = handler(peer_uid, peer_gid, request);
  } catch (...) {
    response = nlohmann::json{{"ok", false},
                              {"message", "Failed to parse helper request."}};
  }
  ipc.send_response(response.dump());
  ipc.close_client();
}

void set_session_state_permissions(const std::string & /*path*/) {
  // No-op on Windows; NTFS ACLs handle file permissions.
}

void setup_daemon_signals() {
  // No-op on Windows; no SIGPIPE equivalent needed.
}

void cleanup_daemon_endpoint(const std::string & /*endpoint*/) {
  // No-op on Windows; named pipes don't leave filesystem artifacts.
}

} // namespace platform
} // namespace ecnuvpn
