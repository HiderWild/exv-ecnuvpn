#include "platform/common/service_status.hpp"

#include "helper.hpp"
#include "platform/common/helper_platform.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <string>
#include <vector>

namespace ecnuvpn {
namespace platform {

ServiceStatusSnapshot current_service_status() {
  const auto &config = helper_platform_config();
  ServiceStatusSnapshot status;
  status.available = helper::is_available();
  status.mode = config.service_mode;
  status.path = config.default_service_binary_path;
  status.endpoint = config.endpoint;
  status.label = config.service_label;

  SC_HANDLE scm = OpenSCManagerA(NULL, NULL, SC_MANAGER_CONNECT);
  if (!scm) {
    status.installed = false;
    status.running = false;
    return status;
  }

    SC_HANDLE svc = OpenServiceA(
      scm, config.service_name, SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG);
  status.installed = (svc != NULL);
  if (!svc) {
    status.running = false;
    CloseServiceHandle(scm);
    return status;
  }

  SERVICE_STATUS service_status;
  if (QueryServiceStatus(svc, &service_status)) {
    status.running = service_status.dwCurrentState == SERVICE_RUNNING;
    status.has_service_state = true;
    status.service_state = static_cast<int>(service_status.dwCurrentState);
  } else {
    status.running = false;
  }

  DWORD bytes_needed = 0;
  QueryServiceConfigA(svc, NULL, 0, &bytes_needed);
  if (bytes_needed > 0) {
    std::vector<unsigned char> buffer(bytes_needed);
    auto *config = reinterpret_cast<QUERY_SERVICE_CONFIGA *>(buffer.data());
    if (QueryServiceConfigA(svc, config, bytes_needed, &bytes_needed) &&
        config->lpBinaryPathName) {
      status.binary_path = std::string(config->lpBinaryPathName);
    }
  }

  CloseServiceHandle(svc);
  CloseServiceHandle(scm);
  return status;
}

} // namespace platform
} // namespace ecnuvpn