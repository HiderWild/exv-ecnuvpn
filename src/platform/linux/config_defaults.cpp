#include "platform/common/config_defaults.hpp"
#include "generated/distribution_config.hpp"

namespace exv {
namespace platform {

const ConfigDefaults &config_defaults() {
  static const ConfigDefaults defaults{
      false,
      std::string(distribution::kDefaultUserAgent),
      "~/.exv/exv.log",
      true,
  };
  return defaults;
}

} // namespace platform
} // namespace exv
