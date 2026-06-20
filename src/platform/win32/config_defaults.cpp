#include "platform/common/config_defaults.hpp"
#include "generated/distribution_config.hpp"

#include <cstdlib>

namespace exv {
namespace platform {
namespace {

std::string default_log_file_path() {
  const char *local_app_data = std::getenv("LOCALAPPDATA");
  if (local_app_data && *local_app_data) {
    return std::string(local_app_data) +
           "\\EXV\\profile\\default\\exv.log";
  }

  const char *home = std::getenv("USERPROFILE");
  if (home && *home) {
    return std::string(home) +
           "\\AppData\\Local\\EXV\\profile\\default\\exv.log";
  }

  return "C:\\ProgramData\\EXV\\profile\\default\\exv.log";
}

} // namespace

const ConfigDefaults &config_defaults() {
  static const ConfigDefaults defaults{
      false,
      std::string(distribution::kDefaultUserAgent),
      default_log_file_path(),
      true,
  };
  return defaults;
}

} // namespace platform
} // namespace exv
