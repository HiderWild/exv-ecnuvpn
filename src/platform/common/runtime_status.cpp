#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/runtime_status.hpp"

#include "platform/common/status_models.hpp"

#ifndef ECNUVPN_VERSION
#define ECNUVPN_VERSION "dev"
#endif

namespace ecnuvpn {
namespace platform {

nlohmann::json runtime_status_json(const ConfigView &cfg) {
  (void)cfg;
  nlohmann::json native{{"engine", "native"},
                        {"mode", "native"},
                        {"available", true},
                        {"source", "native"},
                        {"path", ""},
                        {"version", ECNUVPN_VERSION},
                        {"bundled_runtime_dir", platform::get_bundled_runtime_dir()}};
#ifdef _WIN32
  native["wintun_path"] = platform::get_bundled_wintun_path();
  native["tap_installer_path"] = platform::get_bundled_tap_installer_path();
#endif
  return native;
}

} // namespace platform
} // namespace ecnuvpn
