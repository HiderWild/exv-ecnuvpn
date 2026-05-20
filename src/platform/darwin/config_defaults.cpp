#include "platform/common/config_defaults.hpp"

namespace ecnuvpn {
namespace platform {

const ConfigDefaults &config_defaults() {
  static const ConfigDefaults defaults{
      true,
      "AnyConnect Darwin_x86_64 4.10.05095",
      "~/.ecnuvpn/ecnuvpn.log",
      true,
  };
  return defaults;
}

} // namespace platform
} // namespace ecnuvpn