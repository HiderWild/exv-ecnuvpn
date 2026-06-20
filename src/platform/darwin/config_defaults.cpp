#include "platform/common/config_defaults.hpp"
#include "generated/distribution_config.hpp"

namespace exv {
namespace platform {

const ConfigDefaults &config_defaults() {
  static const ConfigDefaults defaults{
      true,
      std::string(distribution::kDefaultUserAgent),
      "~/Library/Application Support/EXV/profile/default/exv.log",
      true,
  };
  return defaults;
}

} // namespace platform
} // namespace exv
