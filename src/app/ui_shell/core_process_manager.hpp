#pragma once

#include "app/ui_shell/core_rpc_client.hpp"

#include <memory>
#include <string>
#include <vector>

namespace ecnuvpn::ui_shell {

struct CoreProcessLaunch {
  std::string exv_path;
  std::string state_dir;
  std::string runtime_dir;
  bool use_stdin = true;
};

class CoreProcessManager {
public:
  virtual ~CoreProcessManager() = default;
  virtual bool start(const CoreProcessLaunch &launch) = 0;
  virtual void stop() = 0;
  virtual bool alive() const = 0;
};

std::vector<std::string> build_core_process_arguments(
    const CoreProcessLaunch &launch);

void configure_core_process_transport_signal_policy();

std::unique_ptr<CoreRpcTransport> create_core_process_transport(
    const CoreProcessLaunch &launch);

} // namespace ecnuvpn::ui_shell
