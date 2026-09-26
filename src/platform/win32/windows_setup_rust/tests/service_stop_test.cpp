// 直接编译生产停止器，在 Win32 API 边界注入 SCM 与虚拟时钟；不访问真实服务。
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <iostream>
#include <limits>
#include <string>

namespace fixture {
ULONGLONG now = 0;
ULONGLONG stopped_at = 0;
ULONGLONG last_query_at = 0;
DWORD initial_state = SERVICE_RUNNING;
DWORD control_error = ERROR_SUCCESS;
unsigned rejected_requests = 0;
unsigned requests = 0;
unsigned closes = 0;
bool query_fails = false;

SC_HANDLE WINAPI OpenManager(LPCWSTR, LPCWSTR, DWORD) {
  return reinterpret_cast<SC_HANDLE>(1);
}
SC_HANDLE WINAPI OpenService(SC_HANDLE, LPCWSTR, DWORD) {
  return reinterpret_cast<SC_HANDLE>(2);
}
BOOL WINAPI CloseService(SC_HANDLE) { ++closes; return TRUE; }
BOOL WINAPI Query(SC_HANDLE, LPSERVICE_STATUS status) {
  last_query_at = now;
  if (query_fails) { SetLastError(ERROR_ACCESS_DENIED); return FALSE; }
  status->dwCurrentState = now >= stopped_at ? SERVICE_STOPPED : initial_state;
  return TRUE;
}
BOOL WINAPI Stop(SC_HANDLE, DWORD, LPSERVICE_STATUS status) {
  ++requests;
  if (control_error != ERROR_SUCCESS && requests <= rejected_requests) {
    SetLastError(control_error);
    return FALSE;
  }
  status->dwCurrentState = SERVICE_STOP_PENDING;
  initial_state = SERVICE_STOP_PENDING;
  return TRUE;
}
void WINAPI Wait(DWORD milliseconds) { now += milliseconds; }
ULONGLONG WINAPI Clock() { return now; }
void Reset(ULONGLONG terminal_time) {
  now = last_query_at = 0;
  stopped_at = terminal_time;
  initial_state = SERVICE_RUNNING;
  control_error = ERROR_SUCCESS;
  rejected_requests = requests = closes = 0;
  query_fails = false;
}
}

#define OpenSCManagerW fixture::OpenManager
#define OpenServiceW fixture::OpenService
#define CloseServiceHandle fixture::CloseService
#define QueryServiceStatus fixture::Query
#define ControlService fixture::Stop
#define Sleep fixture::Wait
#define GetTickCount64 fixture::Clock
#include "windows_setup_rust/service_control.cpp"
#undef OpenSCManagerW
#undef OpenServiceW
#undef CloseServiceHandle
#undef QueryServiceStatus
#undef ControlService
#undef Sleep
#undef GetTickCount64

namespace exv::setup {
HiddenProcessResult RunHidden(const std::wstring &, const std::wstring &, int) { return {}; }
}

int main() {
  using namespace fixture;
  int failures = 0;
  auto check = [&](bool pass, const char *name) {
    std::cout << (pass ? "PASS: " : "FAIL: ") << name << '\n';
    if (!pass) ++failures;
  };
  // 与 Rust heartbeat::ASSEMBLY_CANCEL_WAIT + service::SERVICE_EXIT_FLUSH + 2s 同语义。
  constexpr ULONGLONG expected_budget = 5000 + 300 + 2000;
  Reset(5300);
  check(exv::setup::StopService() && now == 5300 && requests == 1 && closes == 2,
        "assembly cancellation and flush may finish after the old 5s budget");
  Reset(expected_budget);
  check(exv::setup::StopService() && last_query_at == expected_budget && closes == 2,
        "requery terminal state after the final wait at the deadline");
  Reset(std::numeric_limits<ULONGLONG>::max());
  check(!exv::setup::StopService() && now == expected_budget &&
            last_query_at == expected_budget && requests == 1 && closes == 2,
        "real timeout is bounded and checks the final SCM state");
  Reset(0);
  check(exv::setup::StopService() && now == 0 && requests == 0 && closes == 2,
        "already stopped does not send a control request");
  Reset(200);
  initial_state = SERVICE_STOP_PENDING;
  check(exv::setup::StopService() && requests == 0 && closes == 2,
        "pending stop is observed without another stop request");
  Reset(200);
  control_error = ERROR_SERVICE_CANNOT_ACCEPT_CTRL;
  rejected_requests = 1;
  check(exv::setup::StopService() && requests == 2 && closes == 2,
        "transient 1061 rejection is retried before the deadline");
  Reset(200);
  control_error = ERROR_SERVICE_NOT_ACTIVE;
  rejected_requests = 1;
  check(exv::setup::StopService() && requests == 1 && last_query_at == 200,
        "1062 still requires observed stopped state");
  Reset(std::numeric_limits<ULONGLONG>::max());
  control_error = ERROR_SERVICE_NOT_ACTIVE;
  rejected_requests = 1;
  check(!exv::setup::StopService() && requests == 1 && now == expected_budget,
        "1062 alone cannot report successful stop");
  Reset(100);
  query_fails = true;
  check(!exv::setup::StopService() && requests == 0 && closes == 2,
        "query failure is not mistaken for stopped and closes both handles");
  return failures == 0 ? 0 : 1;
}
