#include "platform/common/config_defaults.hpp"

#include <cstdlib>

namespace ecnuvpn {
namespace platform {
namespace {

std::string default_log_file_path() {
  const char *appdata = std::getenv("APPDATA");
  if (appdata && *appdata)
    return std::string(appdata) + "\\ecnuvpn\\ecnuvpn.log";

  const char *home = std::getenv("USERPROFILE");
  if (home && *home)
    return std::string(home) + "\\AppData\\Roaming\\ecnuvpn\\ecnuvpn.log";

  return "C:\\ProgramData\\ecnuvpn\\ecnuvpn.log";
}

} // namespace

const ConfigDefaults &config_defaults() {
  static const ConfigDefaults defaults{
      false,
      "AnyConnect Win_x86_64 4.10.05095",
      default_log_file_path(),
      true,
  };
  return defaults;
}

} // namespace platform
} // namespace ecnuvpn
