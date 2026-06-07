#include "platform/common/service_status.hpp"

#include "helper.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <filesystem>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace platform {

ServiceStatusSnapshot current_service_status() {
  ServiceStatusSnapshot status;
  status.mode = "windows-service";
  status.path = "C:\\Program Files\\ECNU-VPN\\exv-helper.exe";
  status.endpoint = "\\\\.\\pipe\\exv-helper";
  status.label = "exv-helper";
  status.capabilities = nlohmann::json{{"service_mode", true},
                                       {"oneshot_mode", true},
                                       {"temporary_connect", true},
                                       {"direct_fallback", false},
                                       {"helper_binary", true}};

  SC_HANDLE scm = OpenSCManagerA(NULL, NULL, SC_MANAGER_CONNECT);
  if (!scm) {
    status.installed = false;
    status.running = false;
    return status;
  }

  SC_HANDLE svc = OpenServiceA(
      scm, "exv-helper", SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG);
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
      std::string binary = status.binary_path;
      if (!binary.empty() && binary.front() == '"') {
        const auto end_quote = binary.find('"', 1);
        if (end_quote != std::string::npos)
          status.path = binary.substr(1, end_quote - 1);
      } else {
        const auto arg_pos = binary.find(" --");
        status.path = binary.substr(0, arg_pos);
      }
    }
  }

  CloseServiceHandle(svc);
  CloseServiceHandle(scm);
  status.available = status.running && helper::is_available();
  if (status.installed && !status.path.empty()) {
    std::filesystem::path service_path(status.path);
    if (!std::filesystem::exists(service_path)) {
      status.available = false;
      status.warning =
          "Helper service is registered, but the installed helper binary is "
          "missing. Reinstall the helper service from the desktop package.";
      status.capabilities["service_mode"] = false;
    }
  }
  return status;
}

} // namespace platform
} // namespace ecnuvpn
