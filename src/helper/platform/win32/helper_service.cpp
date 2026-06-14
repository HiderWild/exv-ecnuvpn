#include "helper/helper.hpp"
#include "helper/platform/helper_platform.hpp"
#include "utils.hpp"

#include <string>

#ifdef _WIN32
#include <windows.h>

namespace {

constexpr const char *kServiceName = "exv-helper";

SERVICE_STATUS_HANDLE service_status_handle = nullptr;
SERVICE_STATUS service_status = {};

void report_service_status(DWORD state, DWORD win32_exit_code = NO_ERROR,
                           DWORD wait_hint = 0) {
  if (!service_status_handle)
    return;

  service_status.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
  service_status.dwCurrentState = state;
  service_status.dwWin32ExitCode = win32_exit_code;
  service_status.dwWaitHint = wait_hint;
  service_status.dwControlsAccepted =
      state == SERVICE_RUNNING ? (SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN)
                               : 0;

  static DWORD checkpoint = 1;
  if (state == SERVICE_START_PENDING || state == SERVICE_STOP_PENDING) {
    service_status.dwCheckPoint = checkpoint++;
  } else {
    service_status.dwCheckPoint = 0;
  }

  SetServiceStatus(service_status_handle, &service_status);
}

DWORD WINAPI service_control_handler(DWORD control, DWORD, LPVOID, LPVOID) {
  if (control == SERVICE_CONTROL_STOP || control == SERVICE_CONTROL_SHUTDOWN) {
    report_service_status(SERVICE_STOP_PENDING, NO_ERROR, 3000);
    ecnuvpn::helper::request_daemon_stop();
    return NO_ERROR;
  }
  return ERROR_CALL_NOT_IMPLEMENTED;
}

void WINAPI service_main(DWORD, LPSTR *) {
  service_status_handle =
      RegisterServiceCtrlHandlerExA(kServiceName, service_control_handler, NULL);
  if (!service_status_handle)
    return;

  report_service_status(SERVICE_START_PENDING, NO_ERROR, 3000);
  report_service_status(SERVICE_RUNNING);
  ecnuvpn::helper::DaemonOptions options;
  options.mode = "service";
  options.endpoint = ecnuvpn::platform::helper_platform_config().endpoint;
  int rc = ecnuvpn::helper::daemon_main(options);
  report_service_status(SERVICE_STOPPED, rc == 0 ? NO_ERROR : ERROR_SERVICE_SPECIFIC_ERROR);
}

int run_service() {
  SERVICE_TABLE_ENTRYA table[] = {
      {const_cast<char *>(kServiceName), service_main},
      {NULL, NULL},
  };

  if (StartServiceCtrlDispatcherA(table))
    return 0;

  return 1;
}

} // namespace
#endif

int main(int argc, char *argv[]) {
  ecnuvpn::utils::enable_windows_ansi();

#ifdef _WIN32
  if (argc > 1) {
    std::string arg = argv[1];
    if (arg == "--service")
      return run_service();
  }
  return 2;
#else
  (void)argc;
  (void)argv;
  return 1;
#endif
}
