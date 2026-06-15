#include "app/ui_shell/core_process_manager.hpp"

namespace ecnuvpn::ui_shell {

std::vector<std::string> build_core_process_arguments(
    const CoreProcessLaunch &launch) {
  std::vector<std::string> args{"--mode=core"};
  if (!launch.state_dir.empty()) {
    args.emplace_back("--config-dir");
    args.emplace_back(launch.state_dir);
  }
  if (!launch.runtime_dir.empty()) {
    args.emplace_back("--home");
    args.emplace_back(launch.runtime_dir);
  }
  if (!launch.use_stdin) {
    args.emplace_back("--daemon");
  }
  return args;
}

} // namespace ecnuvpn::ui_shell
