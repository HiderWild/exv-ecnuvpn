#include "windows_setup_rust/service_control.hpp"

#include "windows_setup_rust/util/hidden_process.hpp"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <filesystem>
#include <string>
#include <vector>

#include <cwctype>

namespace exv::setup {
namespace fs = std::filesystem;

namespace {
// 与 Rust engine heartbeat::ASSEMBLY_CANCEL_WAIT、service::SERVICE_EXIT_FLUSH 同语义。
// 两种语言无法直接共享 Duration 常量；service_stop_test 用虚拟时钟钉住预算与末次复查。
constexpr ULONGLONG kAssemblyCancelWaitMs = 5000;
constexpr ULONGLONG kServiceExitFlushMs = 300;
constexpr ULONGLONG kServiceTeardownAllowanceMs = 2000;
constexpr ULONGLONG kServiceStopTimeoutMs =
    kAssemblyCancelWaitMs + kServiceExitFlushMs + kServiceTeardownAllowanceMs;
// 额外 2s 仅是资源收尾/SCM 调度余量，不承诺无界驱动或线程阻塞一定完成。
constexpr DWORD kServiceStopPollMs = 100;
constexpr unsigned kStopControlAttempts = 3;
}  // namespace

ServicePresence QueryServicePresence(const std::wstring &service_name) {
  SC_HANDLE scm = OpenSCManagerW(nullptr, nullptr, SC_MANAGER_CONNECT);
  if (scm == nullptr) {
    return ServicePresence::Error;
  }
  SC_HANDLE svc = OpenServiceW(scm, service_name.c_str(), SERVICE_QUERY_STATUS);
  if (svc != nullptr) {
    CloseServiceHandle(svc);
    CloseServiceHandle(scm);
    return ServicePresence::Installed;
  }
  const DWORD error = GetLastError();
  CloseServiceHandle(scm);
  return error == ERROR_SERVICE_DOES_NOT_EXIST ? ServicePresence::NotInstalled
                                                : ServicePresence::Error;
}

bool IsServiceInstalled(const std::wstring &service_name) {
  // The legacy bool API is conservative: an SCM query error must not be
  // mistaken for absence and must not silently skip privileged cleanup.
  return QueryServicePresence(service_name) != ServicePresence::NotInstalled;
}

ServiceRuntimeSnapshot QueryServiceRuntimeState(const std::wstring &service_name) {
  SC_HANDLE scm = OpenSCManagerW(nullptr, nullptr, SC_MANAGER_CONNECT);
  if (scm == nullptr) {
    return {ServiceRuntimeState::Error, 0, GetLastError()};
  }
  SC_HANDLE svc = OpenServiceW(scm, service_name.c_str(), SERVICE_QUERY_STATUS);
  if (svc == nullptr) {
    const DWORD error = GetLastError();
    CloseServiceHandle(scm);
    return {error == ERROR_SERVICE_DOES_NOT_EXIST ? ServiceRuntimeState::NotInstalled
                                                 : ServiceRuntimeState::Error,
            0, error};
  }
  SERVICE_STATUS status{};
  const BOOL queried = QueryServiceStatus(svc, &status);
  const DWORD error = queried ? ERROR_SUCCESS : GetLastError();
  CloseServiceHandle(svc);
  CloseServiceHandle(scm);
  if (!queried) {
    return {ServiceRuntimeState::Error, 0, error};
  }
  const auto state = status.dwCurrentState == SERVICE_RUNNING ? ServiceRuntimeState::Running
      : status.dwCurrentState == SERVICE_STOPPED ? ServiceRuntimeState::Stopped
                                                 : ServiceRuntimeState::Other;
  return {state, status.dwCurrentState, ERROR_SUCCESS};
}

bool StartServiceAndWait(const std::wstring &service_name, std::wstring *error) {
  auto fail = [&](const wchar_t *step, DWORD code, DWORD state = 0, DWORD service_code = 0) {
    if (error) {
      *error = std::wstring(step) + L" (Win32=" + std::to_wstring(code) +
               L", SCM=" + std::to_wstring(state) + L")";
      if (code == ERROR_SERVICE_SPECIFIC_ERROR) {
        *error += L" ServiceSpecific=" + std::to_wstring(service_code);
      }
    }
    return false;
  };
  SC_HANDLE scm = OpenSCManagerW(nullptr, nullptr, SC_MANAGER_CONNECT);
  if (scm == nullptr) {
    return fail(L"OpenSCManager", GetLastError());
  }
  SC_HANDLE svc = OpenServiceW(scm, service_name.c_str(), SERVICE_START | SERVICE_QUERY_STATUS);
  if (svc == nullptr) {
    const DWORD code = GetLastError();
    CloseServiceHandle(scm);
    return fail(L"OpenService", code);
  }
  // 在同一服务句柄上完成请求和确认，不能把 StartService 返回成功当作运行成功。
  auto start_and_wait = [&]() {
    if (!StartServiceW(svc, 0, nullptr)) {
      const DWORD code = GetLastError();
      if (code != ERROR_SERVICE_ALREADY_RUNNING) {
        return fail(L"StartService", code);
      }
    }
    const ULONGLONG deadline = GetTickCount64() + 15000;
    for (;;) {
      SERVICE_STATUS status{};
      if (!QueryServiceStatus(svc, &status)) {
        return fail(L"QueryServiceStatus", GetLastError());
      }
      if (status.dwCurrentState == SERVICE_RUNNING) {
        return true;
      }
      if (status.dwCurrentState == SERVICE_STOPPED) {
        return fail(L"ServiceStopped", status.dwWin32ExitCode, status.dwCurrentState,
                    status.dwServiceSpecificExitCode);
      }
      if (GetTickCount64() >= deadline) {
        return fail(L"WaitForRunning", ERROR_TIMEOUT, status.dwCurrentState);
      }
      Sleep(100);
    }
  };
  const bool started = start_and_wait();
  CloseServiceHandle(svc);
  CloseServiceHandle(scm);
  return started;
}

bool StopService(const std::wstring &service_name) {
  SC_HANDLE scm = OpenSCManagerW(nullptr, nullptr, SC_MANAGER_ALL_ACCESS);
  if (scm == nullptr) {
    return false;
  }

  SC_HANDLE svc = OpenServiceW(scm, service_name.c_str(), SERVICE_STOP | SERVICE_QUERY_STATUS);
  if (svc == nullptr) {
    const DWORD error = GetLastError();
    CloseServiceHandle(scm);
    return error == ERROR_SERVICE_DOES_NOT_EXIST;
  }

  const bool stopped = [&]() {
    const ULONGLONG deadline = GetTickCount64() + kServiceStopTimeoutMs;
    bool stop_requested = false;
    unsigned stop_attempts = 0;
    for (;;) {
      SERVICE_STATUS status{};
      if (!QueryServiceStatus(svc, &status)) {
        return false;
      }
      if (status.dwCurrentState == SERVICE_STOPPED) {
        return true;
      }
      if (!stop_requested && status.dwCurrentState != SERVICE_STOP_PENDING) {
        ++stop_attempts;
        if (ControlService(svc, SERVICE_CONTROL_STOP, &status)) {
          stop_requested = true;
        } else {
          const DWORD error = GetLastError();
          if (error == ERROR_SERVICE_NOT_ACTIVE) {
            // 1062 不是 Stopped 观测；不重复发 Stop，但仍查询到实际终态。
            stop_requested = true;
          } else if (error != ERROR_SERVICE_CANNOT_ACCEPT_CTRL ||
                     stop_attempts >= kStopControlAttempts) {
            return false;
          }
        }
      }
      // 查询必须先于截止判断；最后一次等待之后仍需复查 SCM，不能丢失终态。
      const ULONGLONG now = GetTickCount64();
      if (now >= deadline) {
        return false;
      }
      const ULONGLONG remaining = deadline - now;
      Sleep(static_cast<DWORD>(remaining < kServiceStopPollMs ? remaining : kServiceStopPollMs));
    }
  }();

  CloseServiceHandle(svc);
  CloseServiceHandle(scm);
  return stopped;
}

bool StopAndDeleteService(const std::wstring &service_name) {
  if (!StopService(service_name)) {
    return QueryServicePresence(service_name) == ServicePresence::NotInstalled;
  }

  SC_HANDLE scm = OpenSCManagerW(nullptr, nullptr, SC_MANAGER_ALL_ACCESS);
  if (scm == nullptr) {
    return false;
  }
  SC_HANDLE svc = OpenServiceW(scm, service_name.c_str(), DELETE);
  if (svc == nullptr) {
    const DWORD error = GetLastError();
    CloseServiceHandle(scm);
    return error == ERROR_SERVICE_DOES_NOT_EXIST;
  }

  const BOOL deleted = DeleteService(svc);
  const DWORD delete_error = GetLastError();
  CloseServiceHandle(svc);
  CloseServiceHandle(scm);

  if (!deleted && delete_error != ERROR_SERVICE_MARKED_FOR_DELETE) {
    return false;
  }
  return WaitForServiceRemoved(service_name);
}

bool WaitForServiceRemoved(const std::wstring &service_name, int max_attempts, int poll_ms) {
  for (int i = 0; i < max_attempts; ++i) {
    switch (QueryServicePresence(service_name)) {
      case ServicePresence::NotInstalled:
        return true;
      case ServicePresence::Error:
        return false;
      case ServicePresence::Installed:
        break;
    }
    Sleep(static_cast<DWORD>(poll_ms));
  }
  return QueryServicePresence(service_name) == ServicePresence::NotInstalled;
}

bool RunEngineServiceUninstall(const std::wstring &engine_exe_path) {
  if (engine_exe_path.empty() || !fs::exists(engine_exe_path)) {
    return false;
  }
  // Rust engine --service-uninstall removes the SCM exv-engine service + PSK.
  const std::wstring cmd = L"\"" + engine_exe_path + L"\" --service-uninstall";
  const auto r = RunHidden(L"", cmd, 60000);
  return r.started && r.exit_code == 0;
}

// ---- S5（2026-10-10 换盘服务迁移）----

ServiceImagePathParts SplitServiceImagePathExecutable(const std::wstring &image_path) {
  const std::size_t begin = image_path.find_first_not_of(L" \t");
  if (begin == std::wstring::npos) {
    return {};
  }
  if (image_path[begin] == L'"') {
    const std::size_t close = image_path.find(L'"', begin + 1);
    // 未闭合引号容错：引号后全部视为可执行段（比对失败只会多一次幂等改写）。
    if (close == std::wstring::npos) {
      return {image_path.substr(begin + 1), L""};
    }
    ServiceImagePathParts parts;
    parts.executable = image_path.substr(begin + 1, close - begin - 1);
    const std::size_t rest = image_path.find_first_not_of(L" \t", close + 1);
    parts.arguments = rest == std::wstring::npos ? L"" : image_path.substr(rest);
    return parts;
  }
  const std::size_t end = image_path.find_first_of(L" \t", begin);
  ServiceImagePathParts parts;
  parts.executable =
      end == std::wstring::npos ? image_path.substr(begin)
                                : image_path.substr(begin, end - begin);
  const std::size_t rest = end == std::wstring::npos ? std::wstring::npos
                                                     : image_path.find_first_not_of(L" \t", end);
  parts.arguments = rest == std::wstring::npos ? L"" : image_path.substr(rest);
  return parts;
}

std::wstring NormalizeServiceImagePathForCompare(const std::wstring &path) {
  std::size_t begin = path.find_first_not_of(L" \t");
  if (begin == std::wstring::npos) {
    return {};
  }
  const std::size_t end = path.find_last_not_of(L" \t");
  std::wstring normalized = path.substr(begin, end - begin + 1);
  // 去包围引号（成对或单侧残余都规整掉——只影响比对形态）。
  if (!normalized.empty() && normalized.front() == L'"') {
    normalized.erase(0, 1);
  }
  if (!normalized.empty() && normalized.back() == L'"') {
    normalized.pop_back();
  }
  for (wchar_t &ch : normalized) {
    if (ch == L'/') {
      ch = L'\\';
    } else {
      ch = static_cast<wchar_t>(std::towlower(ch));
    }
  }
  return normalized;
}

bool ServiceImagePathDrifted(const std::wstring &image_path,
                             const std::wstring &expected_engine_path) {
  return NormalizeServiceImagePathForCompare(
             SplitServiceImagePathExecutable(image_path).executable) !=
         NormalizeServiceImagePathForCompare(expected_engine_path);
}

ServiceImagePathSnapshot QueryServiceImagePath(const std::wstring &service_name) {
  SC_HANDLE scm = OpenSCManagerW(nullptr, nullptr, SC_MANAGER_CONNECT);
  if (scm == nullptr) {
    return {ServicePresence::Error, L"", GetLastError()};
  }
  SC_HANDLE svc = OpenServiceW(scm, service_name.c_str(), SERVICE_QUERY_CONFIG);
  if (svc == nullptr) {
    const DWORD error = GetLastError();
    CloseServiceHandle(scm);
    return {error == ERROR_SERVICE_DOES_NOT_EXIST ? ServicePresence::NotInstalled
                                                  : ServicePresence::Error,
            L"", error};
  }
  DWORD needed = 0;
  std::vector<BYTE> buffer;
  if (QueryServiceConfigW(svc, nullptr, 0, &needed) || needed == 0) {
    // 预期失败形态是 ERROR_INSUFFICIENT_BUFFER；其余（含诡异的 0 需求）走错误路径。
    const DWORD error = GetLastError();
    CloseServiceHandle(svc);
    CloseServiceHandle(scm);
    return {ServicePresence::Error, L"", error};
  }
  buffer.resize(needed);
  auto *config = reinterpret_cast<QUERY_SERVICE_CONFIGW *>(buffer.data());
  if (!QueryServiceConfigW(svc, config, needed, &needed)) {
    const DWORD error = GetLastError();
    CloseServiceHandle(svc);
    CloseServiceHandle(scm);
    return {ServicePresence::Error, L"", error};
  }
  ServiceImagePathSnapshot snapshot;
  snapshot.presence = ServicePresence::Installed;
  snapshot.binary_path = config->lpBinaryPathName != nullptr ? config->lpBinaryPathName : L"";
  snapshot.error_code = ERROR_SUCCESS;
  CloseServiceHandle(svc);
  CloseServiceHandle(scm);
  return snapshot;
}

bool MigrateServiceImagePath(const std::wstring &new_engine_path,
                             const std::wstring &service_name, std::wstring *error) {
  auto fail = [&](const wchar_t *step, DWORD code) {
    if (error) {
      *error = std::wstring(step) + L" (Win32=" + std::to_wstring(code) + L")";
    }
    return false;
  };
  if (new_engine_path.empty()) {
    return fail(L"empty new image path", ERROR_INVALID_PARAMETER);
  }
  // 尾参保真：控制面管道名/dll/SID/config-dir 是按用户派生的安装目录无关值，
  // 只改写可执行段（复审 MINOR-2 引号/斜杠/大小写规整只影响比对，不回写规整值）。
  const auto current = QueryServiceImagePath(service_name);
  if (current.presence != ServicePresence::Installed) {
    return fail(L"QueryImagePath", current.error_code != 0 ? current.error_code
                                                           : static_cast<DWORD>(ERROR_FILE_NOT_FOUND));
  }
  const ServiceImagePathParts parts = SplitServiceImagePathExecutable(current.binary_path);
  // 写入带引号惯例形态（CreateService 同惯例，路径含空格时必需）。
  std::wstring new_binary_path = L"\"" + new_engine_path + L"\"";
  if (!parts.arguments.empty()) {
    new_binary_path += L" " + parts.arguments;
  }

  SC_HANDLE scm = OpenSCManagerW(nullptr, nullptr, SC_MANAGER_CONNECT);
  if (scm == nullptr) {
    return fail(L"OpenSCManager", GetLastError());
  }
  SC_HANDLE svc = OpenServiceW(scm, service_name.c_str(),
                               SERVICE_CHANGE_CONFIG | SERVICE_QUERY_CONFIG);
  if (svc == nullptr) {
    const DWORD code = GetLastError();
    CloseServiceHandle(scm);
    return fail(L"OpenService", code);
  }
  const BOOL changed = ChangeServiceConfigW(
      svc, SERVICE_NO_CHANGE, SERVICE_NO_CHANGE, SERVICE_NO_CHANGE, new_binary_path.c_str(),
      nullptr, nullptr, nullptr, nullptr, nullptr, nullptr);
  if (!changed) {
    const DWORD code = GetLastError();
    CloseServiceHandle(svc);
    CloseServiceHandle(scm);
    return fail(L"ChangeServiceConfig", code);
  }
  CloseServiceHandle(svc);
  CloseServiceHandle(scm);

  // 写后回读校验（fail-closed）：SCM 报成功但可执行段未按规整语义落地即失败。
  const auto verify = QueryServiceImagePath(service_name);
  if (verify.presence != ServicePresence::Installed ||
      ServiceImagePathDrifted(verify.binary_path, new_engine_path)) {
    return fail(L"VerifyImagePath", ERROR_INVALID_DATA);
  }
  return true;
}

}  // namespace exv::setup
