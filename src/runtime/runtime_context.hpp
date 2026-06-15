#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#pragma once

#include <string>

namespace ecnuvpn {
namespace runtime {

// Resolved, process-wide runtime paths. Pinned once by bootstrap() so that
// every process (CLI, elevated desktop-rpc, helper worker, supervisor) writes
// to the same state directory and log file regardless of how its ambient
// environment resolves %APPDATA% / $HOME.
struct RuntimePaths {
  std::string state_dir;   // canonical config/state directory
  std::string home;        // home directory used to derive state_dir
  std::string config_path; // <state_dir>/config(.json)
  std::string log_path;    // <state_dir>/ecnuvpn.log
};

// Pin the runtime paths for this process. Precedence (highest first):
//   1. explicit_state_dir argument (non-empty)
//   2. environment variable ECNUVPN_STATE_DIR
//   3. an override already installed via platform::set_runtime_path_override
//   4. platform default (derived from the effective home)
//
// Safe to call multiple times; the first successful, non-default resolution
// wins and later calls are ignored unless force is set. Must be called before
// the first log line in every entry point.
void bootstrap(const std::string &explicit_state_dir = "",
               const std::string &home_hint = "", bool force = false);

// Snapshot of the currently pinned paths (always reflects utils getters).
RuntimePaths paths();

// True once bootstrap() has pinned a deterministic state directory.
bool is_bootstrapped();

} // namespace runtime
} // namespace ecnuvpn
