#pragma once

#include "core/tunnel_controller/core_session_runner.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"
#include "vpn_engine/session_state.hpp"

#include <memory>

namespace exv::core {

class TunnelControllerTestAccess {
public:
  static std::unique_ptr<TunnelController> create(
      std::shared_ptr<exv::helper::HelperClient> helper,
      std::shared_ptr<exv::platform::PlatformNetworkOps> net_ops,
      ReconnectConfig reconnect_config,
      CoreSessionRunner::NativeDependenciesFactory deps_factory);

  static ecnuvpn::vpn_engine::ValidationResult configure_network_for_engine(
      TunnelController &controller,
      const ecnuvpn::vpn_engine::TunnelMetadata &metadata,
      ecnuvpn::vpn_engine::DeviceConfig *device_config);
};

} // namespace exv::core
