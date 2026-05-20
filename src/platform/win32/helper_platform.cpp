#include "platform/common/helper_platform.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

namespace ecnuvpn {
namespace platform {

const HelperPlatformConfig &helper_platform_config() {
  static const HelperPlatformConfig config{
      "exv-helper",
      "exv-helper",
      "",
      "\\\\.\\pipe\\exv-helper",
      "C:\\ProgramData\\exv-helper-session.json",
      "C:\\Program Files\\ECNU-VPN\\exv.exe",
      "C:\\Program Files\\ECNU-VPN\\exv-helper.exe",
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
} // namespace ecnuvpn