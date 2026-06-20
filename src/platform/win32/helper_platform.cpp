#include "platform/common/helper_platform.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <cstdlib>
#include <string>

namespace exv {
namespace platform {
namespace {

std::string join_windows_path(const std::string &base,
                              const std::string &component) {
  if (base.empty())
    return component;
  if (base.back() == '\\' || base.back() == '/')
    return base + component;
  return base + "\\" + component;
}

std::string local_app_data_root() {
  const char *local_app_data = std::getenv("LOCALAPPDATA");
  if (local_app_data && *local_app_data)
    return local_app_data;

  const char *user_profile = std::getenv("USERPROFILE");
  if (user_profile && *user_profile) {
    return join_windows_path(join_windows_path(user_profile, "AppData"),
                             "Local");
  }

  const char *program_data = std::getenv("ProgramData");
  if (program_data && *program_data)
    return program_data;

  return "C:\\ProgramData";
}

std::string stable_helper_path() {
  return join_windows_path(
      join_windows_path(join_windows_path(local_app_data_root(), "EXV"),
                        "Helper"),
      "exv-helper.exe");
}

} // namespace

const HelperPlatformConfig &helper_platform_config() {
  static const std::string service_helper_path = stable_helper_path();
  static const HelperPlatformConfig config{
      "exv-helper",
      "exv-helper",
      "",
      "\\\\.\\pipe\\exv-helper",
      "C:\\ProgramData\\exv-helper-session.json",
      "C:\\Program Files\\EXV\\exv.exe",
      service_helper_path.c_str(),
      "windows-service",
  };
  return config;
}

void wake_helper_daemon_for_shutdown() {
  const auto &config = helper_platform_config();
  if (WaitNamedPipeA(config.endpoint, 200)) {
    HANDLE hPipe = CreateFileA(config.endpoint, GENERIC_READ | GENERIC_WRITE, 0,
                               NULL, OPEN_EXISTING, 0, NULL);
    if (hPipe != INVALID_HANDLE_VALUE)
      CloseHandle(hPipe);
  }
}

} // namespace platform
} // namespace exv
