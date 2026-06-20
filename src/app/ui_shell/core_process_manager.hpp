#pragma once

#include "app/ui_shell/core_rpc_client.hpp"
#include "platform/common/core_resolver.hpp"

#include <memory>
#include <string>
#include <vector>

namespace exv::ui_shell {

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

// Classify the current core state using the shared resolver.
// Returns a CoreResolveResult that the UI shell can use to decide
// whether to reuse, launch, or report an error.
// The UI must NOT kill a live residual core automatically.
exv::core::lifecycle::CoreResolveResult
classify_core_state(const exv::core::lifecycle::CoreResolveOptions &options = {},
                    const exv::core::lifecycle::CoreResolverDeps &deps = {});

std::vector<std::string> build_core_process_arguments(
    const CoreProcessLaunch &launch);

void configure_core_process_transport_signal_policy();

std::unique_ptr<CoreRpcTransport> create_core_process_transport(
    const CoreProcessLaunch &launch);

} // namespace exv::ui_shell
