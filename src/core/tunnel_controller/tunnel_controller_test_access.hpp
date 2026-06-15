#pragma once

#include "core/tunnel_controller/core_session_runner.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"

#include <memory>

namespace exv::core {

class TunnelControllerTestAccess {
public:
  static std::unique_ptr<TunnelController> create(
      std::shared_ptr<exv::helper::HelperClient> helper,
      std::shared_ptr<exv::platform::PlatformNetworkOps> net_ops,
      ReconnectConfig reconnect_config,
      CoreSessionRunner::NativeDependenciesFactory deps_factory);
};

} // namespace exv::core
