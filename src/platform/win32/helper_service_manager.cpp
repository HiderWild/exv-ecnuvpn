#include "platform/common/helper_service_manager.hpp"

#include "logger.hpp"
#include "platform/common/helper_platform.hpp"
#include "utils.hpp"

#include <filesystem>
#include <iostream>
#include <string>

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

namespace ecnuvpn {
namespace platform {
namespace {

bool wait_until_ready(const HelperServiceManagerContext &context, int attempts,
                     unsigned int delay_us) {
  return context.wait_until_available &&
         context.wait_until_available(attempts, delay_us);
}

bool send_helper_request(const HelperServiceManagerContext &context,
                         const nlohmann::json &request,
                         nlohmann::json *response,
                         std::string *error_message,
                         int timeout_seconds = 15) {
  return context.send_request &&
         context.send_request(request, response, error_message,
                              timeout_seconds);
}

void print_runtime_status_if_available(const HelperServiceManagerContext &context,
                                       bool available) {
  if (!available)
    return;

  nlohmann::json response;
  std::string error_message;
  if (send_helper_request(context, nlohmann::json{{"action", "status"}},
                          &response, &error_message) &&
      response.value("ok", false)) {
    std::cout << "  VPN Running     : "
              << (response.value("running", false) ? "yes" : "no")
              << std::endl;
    if (response.value("running", false)) {
      std::cout << "  Session Owner   : "
                << response.value("owner_username", std::string())
                << std::endl;
    }
  }
}

} // namespace

int install_helper_service(const std::string &executable_path,
                           const HelperServiceManagerContext &context) {
  const auto &platform_config = helper_platform_config();

  if (!utils::check_root()) {
    utils::print_error(
        "Administrator privileges required. Please run from an elevated prompt.");
    return 1;
  }

  utils::print_info("Opening Service Control Manager...");
  SC_HANDLE hSCM = OpenSCManagerA(NULL, NULL, SC_MANAGER_CREATE_SERVICE);
  if (!hSCM) {
    logger::error("Cannot open Service Control Manager");
    return 1;
  }

  std::string exec_path = executable_path.empty() ? utils::get_executable_path()
                                                  : executable_path;
  if (exec_path.empty()) {
    logger::error("Failed to resolve the exv executable path.");
    CloseServiceHandle(hSCM);
    return 1;
  }

  std::filesystem::path exec_fs_path(exec_path);
  std::filesystem::path helper_path =
      exec_fs_path.parent_path() / "exv-helper.exe";

  std::string binary_path;
  if (std::filesystem::exists(helper_path)) {
    binary_path = "\"" + helper_path.string() + "\" --service";
  } else {
    utils::print_warning(
        "Dedicated exv-helper.exe was not found next to exv.exe. Falling back to legacy in-process helper service mode.");
    binary_path = "\"" + exec_path + "\" __helper-daemon";
  }

  utils::print_info("Registering helper service...");
  SC_HANDLE hService = CreateServiceA(
      hSCM, platform_config.service_name, "ECNU VPN Helper",
      SERVICE_ALL_ACCESS, SERVICE_WIN32_OWN_PROCESS, SERVICE_AUTO_START,
      SERVICE_ERROR_NORMAL, binary_path.c_str(), NULL, NULL, NULL, NULL,
      NULL);

  if (!hService) {
    DWORD err = GetLastError();
    if (err == ERROR_SERVICE_EXISTS) {
      utils::print_info(
          "Helper service is already installed. Refreshing service configuration...");
      hService = OpenServiceA(hSCM, platform_config.service_name,
                              SERVICE_CHANGE_CONFIG | SERVICE_QUERY_STATUS |
                                  SERVICE_START | SERVICE_STOP);
      if (!hService) {
        logger::error("OpenService failed: " +
                      std::to_string(GetLastError()));
        CloseServiceHandle(hSCM);
        return 1;
      }

      if (!ChangeServiceConfigA(hService, SERVICE_NO_CHANGE,
                                SERVICE_NO_CHANGE, SERVICE_NO_CHANGE,
                                binary_path.c_str(), NULL, NULL, NULL, NULL,
                                NULL, NULL)) {
        logger::error("ChangeServiceConfig failed: " +
                      std::to_string(GetLastError()));
        CloseServiceHandle(hService);
        CloseServiceHandle(hSCM);
        return 1;
      }

      SERVICE_STATUS service_status = {};
      if (QueryServiceStatus(hService, &service_status) &&
          service_status.dwCurrentState != SERVICE_STOPPED) {
        utils::print_info(
            "Restarting helper service to apply the new binary path...");
        ControlService(hService, SERVICE_CONTROL_STOP, &service_status);
        for (int i = 0; i < 50; ++i) {
          if (!QueryServiceStatus(hService, &service_status) ||
              service_status.dwCurrentState == SERVICE_STOPPED) {
            break;
          }
          Sleep(100);
        }
      }
    } else {
      logger::error("CreateService failed: " + std::to_string(err));
      CloseServiceHandle(hSCM);
      return 1;
    }
  }

  utils::print_info("Starting helper service...");
  if (!StartService(hService, 0, NULL)) {
    DWORD err = GetLastError();
    if (err != ERROR_SERVICE_ALREADY_RUNNING) {
      logger::error("StartService failed: " + std::to_string(err));
      CloseServiceHandle(hService);
      CloseServiceHandle(hSCM);
      return 1;
    }
  }
  CloseServiceHandle(hService);
  CloseServiceHandle(hSCM);

  utils::print_info("Waiting for helper to become ready...");
  bool helper_ready = wait_until_ready(context, 50, 100000);

  utils::print_success("EXV helper service installed.");
  if (!helper_ready) {
    utils::print_warning(
        "Helper service was installed, but it has not responded yet.");
    utils::print_info("Run 'exv service status' again in a moment if needed.");
  }
  utils::print_info("You can now run 'exv' and 'exv stop' without elevation.");
  return 0;
}

int uninstall_helper_service(const HelperServiceManagerContext &context) {
  const auto &platform_config = helper_platform_config();
  (void)context;

  SC_HANDLE hSCM = OpenSCManagerA(NULL, NULL, SC_MANAGER_CONNECT);
  if (!hSCM)
    return 1;

  SC_HANDLE hService =
      OpenServiceA(hSCM, platform_config.service_name, SERVICE_STOP | DELETE);
  if (!hService) {
    std::cout << "Helper service is not installed.\n";
    CloseServiceHandle(hSCM);
    return 0;
  }

  SERVICE_STATUS status = {};
  ControlService(hService, SERVICE_CONTROL_STOP, &status);
  DeleteService(hService);

  CloseServiceHandle(hService);
  CloseServiceHandle(hSCM);

  std::cout << "Helper service uninstalled.\n";
  return 0;
}

int show_helper_service_status(const HelperServiceManagerContext &context) {
  const auto &platform_config = helper_platform_config();

  utils::print_header("EXV Service Status");

  SC_HANDLE hSCM = OpenSCManagerA(NULL, NULL, SC_MANAGER_CONNECT);
  if (!hSCM) {
    std::cout << "  Installed       : unknown (cannot open SCM)\n";
    return 1;
  }

  SC_HANDLE hService =
      OpenServiceA(hSCM, platform_config.service_name, SERVICE_QUERY_STATUS);
  bool installed = (hService != NULL);
  std::cout << "  Installed       : " << (installed ? "yes" : "no")
            << std::endl;

  bool available = false;
  if (installed) {
    SERVICE_STATUS status = {};
    QueryServiceStatus(hService, &status);
    std::cout << "  State           : "
              << (status.dwCurrentState == SERVICE_RUNNING ? "running"
                                                           : "stopped")
              << std::endl;
    if (status.dwCurrentState == SERVICE_RUNNING)
      available = wait_until_ready(context, 10, 100000);
    CloseServiceHandle(hService);
  }

  std::cout << "  Socket Ready    : " << (available ? "yes" : "no")
            << std::endl;
  print_runtime_status_if_available(context, available);
  std::cout << std::endl;

  CloseServiceHandle(hSCM);
  return 0;
}

} // namespace platform
} // namespace ecnuvpn