#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/helper_service_manager.hpp"

#include "observability/log_facade.hpp"
#include "platform/common/helper_platform.hpp"
#include "cli/console.hpp"

#include <algorithm>
#include <array>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <vector>

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

namespace exv {
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
  (void)context;
  if (!available)
    return;
}

bool files_have_same_contents(const std::filesystem::path &left,
                              const std::filesystem::path &right) {
  std::error_code ec;
  if (!std::filesystem::is_regular_file(left, ec) ||
      !std::filesystem::is_regular_file(right, ec)) {
    return false;
  }

  const auto left_size = std::filesystem::file_size(left, ec);
  if (ec)
    return false;
  const auto right_size = std::filesystem::file_size(right, ec);
  if (ec || left_size != right_size)
    return false;

  std::ifstream left_stream(left, std::ios::binary);
  std::ifstream right_stream(right, std::ios::binary);
  if (!left_stream || !right_stream)
    return false;

  std::array<char, 65536> left_buffer{};
  std::array<char, 65536> right_buffer{};
  while (left_stream && right_stream) {
    left_stream.read(left_buffer.data(), left_buffer.size());
    right_stream.read(right_buffer.data(), right_buffer.size());
    if (left_stream.gcount() != right_stream.gcount())
      return false;
    if (!std::equal(left_buffer.begin(),
                    left_buffer.begin() + left_stream.gcount(),
                    right_buffer.begin())) {
      return false;
    }
  }
  return left_stream.eof() && right_stream.eof();
}

bool ensure_stable_helper_binary(const std::filesystem::path &source,
                                 const std::filesystem::path &target,
                                 bool *refreshed) {
  if (refreshed)
    *refreshed = false;

  std::error_code ec;
  if (!std::filesystem::is_regular_file(source, ec)) {
    cli::print_error("Dedicated exv-helper.exe was not found next to exv.exe.");
    return false;
  }

  if (std::filesystem::equivalent(source, target, ec)) {
    return true;
  }

  std::filesystem::create_directories(target.parent_path(), ec);
  if (ec) {
    cli::print_error("Failed to create stable helper directory: " +
                     ec.message());
    return false;
  }

  if (files_have_same_contents(source, target)) {
    return true;
  }

  std::filesystem::copy_file(source, target,
                             std::filesystem::copy_options::overwrite_existing,
                             ec);
  if (ec) {
    cli::print_error("Failed to refresh stable exv-helper.exe: " +
                     ec.message());
    return false;
  }
  if (refreshed)
    *refreshed = true;
  return true;
}

std::string service_binary_path(SC_HANDLE service) {
  DWORD bytes_needed = 0;
  QueryServiceConfigA(service, NULL, 0, &bytes_needed);
  if (bytes_needed == 0)
    return {};

  std::vector<unsigned char> buffer(bytes_needed);
  auto *config = reinterpret_cast<QUERY_SERVICE_CONFIGA *>(buffer.data());
  if (!QueryServiceConfigA(service, config, bytes_needed, &bytes_needed) ||
      !config->lpBinaryPathName) {
    return {};
  }
  return config->lpBinaryPathName;
}

} // namespace

int install_helper_service(const std::string &executable_path,
                           const HelperServiceManagerContext &context) {
  const auto &platform_config = helper_platform_config();

  if (!platform::check_root()) {
    cli::print_error(
        "Administrator privileges required. Please run from an elevated prompt.");
    return 1;
  }

  cli::print_info("Opening Service Control Manager...");
  SC_HANDLE hSCM = OpenSCManagerA(NULL, NULL, SC_MANAGER_CREATE_SERVICE);
  if (!hSCM) {
    exv::observability::LogFacade::error("Cannot open Service Control Manager");
    return 1;
  }

  std::string exec_path = executable_path.empty() ? platform::get_executable_path()
                                                  : executable_path;
  if (exec_path.empty()) {
    exv::observability::LogFacade::error("Failed to resolve the exv executable path.");
    CloseServiceHandle(hSCM);
    return 1;
  }

  std::filesystem::path exec_fs_path(exec_path);
  const std::filesystem::path packaged_helper_path =
      exec_fs_path.parent_path() / "exv-helper.exe";
  const std::filesystem::path stable_helper_path =
      platform_config.default_service_binary_path;

  bool helper_refreshed = false;
  if (!ensure_stable_helper_binary(packaged_helper_path, stable_helper_path,
                                   &helper_refreshed)) {
    CloseServiceHandle(hSCM);
    return 1;
  }

  std::string binary_path = "\"" + stable_helper_path.string() + "\" --service";

  cli::print_info("Registering helper service...");
  SC_HANDLE hService = CreateServiceA(
      hSCM, platform_config.service_name, "EXV Helper",
      SERVICE_ALL_ACCESS, SERVICE_WIN32_OWN_PROCESS, SERVICE_AUTO_START,
      SERVICE_ERROR_NORMAL, binary_path.c_str(), NULL, NULL, NULL, NULL,
      NULL);

  if (!hService) {
    DWORD err = GetLastError();
    if (err == ERROR_SERVICE_EXISTS) {
      cli::print_info(
          "Helper service is already installed. Refreshing service configuration...");
      hService = OpenServiceA(hSCM, platform_config.service_name,
                              SERVICE_CHANGE_CONFIG | SERVICE_QUERY_STATUS |
                                  SERVICE_START | SERVICE_STOP);
      if (!hService) {
        exv::observability::LogFacade::error("OpenService failed: " +
                      std::to_string(GetLastError()));
        CloseServiceHandle(hSCM);
        return 1;
      }

      bool service_config_changed = false;
      if (service_binary_path(hService) != binary_path) {
        if (!ChangeServiceConfigA(hService, SERVICE_NO_CHANGE,
                                  SERVICE_NO_CHANGE, SERVICE_NO_CHANGE,
                                  binary_path.c_str(), NULL, NULL, NULL, NULL,
                                  NULL, NULL)) {
          exv::observability::LogFacade::error("ChangeServiceConfig failed: " +
                        std::to_string(GetLastError()));
          CloseServiceHandle(hService);
          CloseServiceHandle(hSCM);
          return 1;
        }
        service_config_changed = true;
      }

      SERVICE_STATUS service_status = {};
      if (QueryServiceStatus(hService, &service_status) &&
          service_status.dwCurrentState != SERVICE_STOPPED &&
          (service_config_changed || helper_refreshed)) {
        cli::print_info(
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
      exv::observability::LogFacade::error("CreateService failed: " + std::to_string(err));
      CloseServiceHandle(hSCM);
      return 1;
    }
  }

  cli::print_info("Starting helper service...");
  if (!StartService(hService, 0, NULL)) {
    DWORD err = GetLastError();
    if (err != ERROR_SERVICE_ALREADY_RUNNING) {
      exv::observability::LogFacade::error("StartService failed: " + std::to_string(err));
      CloseServiceHandle(hService);
      CloseServiceHandle(hSCM);
      return 1;
    }
  }
  CloseServiceHandle(hService);
  CloseServiceHandle(hSCM);

  cli::print_info("Waiting for helper to become ready...");
  bool helper_ready = wait_until_ready(context, 50, 100000);

  cli::print_success("EXV helper service installed.");
  if (!helper_ready) {
    cli::print_warning(
        "Helper service was installed, but it has not responded yet.");
    cli::print_info("Run 'exv service status' again in a moment if needed.");
  }
  cli::print_info("You can now run 'exv' and 'exv stop' without elevation.");
  return 0;
}

int uninstall_helper_service(const HelperServiceManagerContext &context) {
  const auto &platform_config = helper_platform_config();

  SC_HANDLE hSCM = OpenSCManagerA(NULL, NULL, SC_MANAGER_CONNECT);
  if (!hSCM)
    return 1;

  SC_HANDLE hService = OpenServiceA(
      hSCM, platform_config.service_name,
      SERVICE_STOP | SERVICE_QUERY_STATUS | DELETE);
  if (!hService) {
    std::cout << "Helper service is not installed.\n";
    CloseServiceHandle(hSCM);
    return 0;
  }

  SERVICE_STATUS status = {};
  if (QueryServiceStatus(hService, &status) &&
      status.dwCurrentState != SERVICE_STOPPED) {
    std::cout << "Stopping helper service...\n";
    ControlService(hService, SERVICE_CONTROL_STOP, &status);
    for (int i = 0; i < 100; ++i) {
      if (!QueryServiceStatus(hService, &status) ||
          status.dwCurrentState == SERVICE_STOPPED) {
        break;
      }
      Sleep(100);
    }
  }

  if (context.clear_session_state)
    context.clear_session_state();

  std::cout << "Deleting helper service registration...\n";
  if (!DeleteService(hService)) {
    DWORD err = GetLastError();
    if (err != ERROR_SERVICE_MARKED_FOR_DELETE) {
      exv::observability::LogFacade::error("DeleteService failed: " + std::to_string(err));
      CloseServiceHandle(hService);
      CloseServiceHandle(hSCM);
      return 1;
    }
  }

  CloseServiceHandle(hService);

  bool removed = false;
  for (int i = 0; i < 50; ++i) {
    SC_HANDLE check =
        OpenServiceA(hSCM, platform_config.service_name, SERVICE_QUERY_STATUS);
    if (!check) {
      removed = GetLastError() == ERROR_SERVICE_DOES_NOT_EXIST;
      if (removed)
        break;
    } else {
      CloseServiceHandle(check);
    }
    Sleep(100);
  }

  CloseServiceHandle(hSCM);

  if (!removed) {
    exv::observability::LogFacade::error("Helper service is still registered after uninstall.");
    return 1;
  }

  std::cout << "Helper service uninstalled.\n";
  return 0;
}

int show_helper_service_status(const HelperServiceManagerContext &context) {
  const auto &platform_config = helper_platform_config();

  cli::print_header("EXV Service Status");

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
} // namespace exv
