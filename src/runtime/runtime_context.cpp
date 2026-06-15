#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "runtime/runtime_context.hpp"


#include <cstdlib>

namespace ecnuvpn {
namespace runtime {

namespace {
bool g_bootstrapped = false;

std::string env_state_dir() {
  const char *value = std::getenv("ECNUVPN_STATE_DIR");
  if (value && *value) {
    return std::string(value);
  }
  return std::string();
}
} // namespace

void bootstrap(const std::string &explicit_state_dir,
               const std::string &home_hint, bool force) {
  if (g_bootstrapped && !force) {
    return;
  }

  std::string state_dir = explicit_state_dir;
  if (state_dir.empty()) {
    state_dir = env_state_dir();
  }

  if (!state_dir.empty()) {
    // Pin both home and config dir so every path getter resolves identically.
    const std::string home =
        home_hint.empty() ? platform::get_effective_home() : home_hint;
    platform::set_runtime_path_override(home, state_dir);
    g_bootstrapped = true;
    return;
  }

  if (!home_hint.empty()) {
    platform::set_runtime_path_override(home_hint, "");
    g_bootstrapped = true;
    return;
  }

  // No explicit/env/hint state dir: keep whatever override (if any) the caller
  // already installed and fall back to the platform default otherwise. Mark as
  // bootstrapped so repeated calls are cheap, but do not force a default path
  // that could differ from an override installed later in the same process.
  g_bootstrapped = true;
}

RuntimePaths paths() {
  RuntimePaths p;
  p.state_dir = platform::get_config_dir();
  p.home = platform::get_effective_home();
  p.config_path = platform::get_config_path();
  p.log_path = platform::get_log_path();
  return p;
}

bool is_bootstrapped() { return g_bootstrapped; }

} // namespace runtime
} // namespace ecnuvpn
