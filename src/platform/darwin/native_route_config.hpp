#pragma once

#include "vpn_engine/session_state.hpp"

#include <cstdint>
#include <functional>
#include <string>
#include <vector>

namespace exv {
namespace platform {

enum class NativeDarwinRouteConfigError {
  none,
  api_missing,
  invalid_interface,
  invalid_mtu,
  invalid_route,
  mtu_set_failed,
  upstream_route_failed,
  route_add_failed,
  route_delete_failed,
};

const char *
native_darwin_route_config_error_code(NativeDarwinRouteConfigError error);

struct NativeDarwinRouteConfigResult {
  NativeDarwinRouteConfigError error = NativeDarwinRouteConfigError::none;
  std::string message;
  std::string target;
  int system_error = 0;
  int effective_mtu = 0;

  bool ok() const { return error == NativeDarwinRouteConfigError::none; }
};

struct NativeDarwinUpstreamRoute {
  std::string interface_name;
  std::uint32_t interface_index = 0;
  std::string gateway;
};

struct NativeDarwinRoute {
  std::string cidr;
  std::string destination;
  std::string netmask;
  int prefix_length = 32;
  std::string interface_name;
  std::uint32_t interface_index = 0;
  std::uint32_t message_interface_index = 0;
  bool message_interface_scoped = false;
  std::string gateway;
  bool server_bypass = false;
};

struct NativeDarwinRouteApi {
  using ErrorCode = int;

  std::function<ErrorCode(const std::string &, int)> set_interface_mtu;
  std::function<ErrorCode(const std::string &, NativeDarwinUpstreamRoute &)>
      get_best_route;
  std::function<ErrorCode(const NativeDarwinRoute &)> add_route;
  std::function<ErrorCode(const NativeDarwinRoute &)> delete_route;
  std::function<std::uint32_t(const std::string &)> interface_index_from_name;
};

NativeDarwinRouteApi default_native_darwin_route_api();

struct NativeDarwinRouteConfigOptions {
  std::string interface_name;
  std::uint32_t interface_index = 0;
  int configured_mtu = 1290;
};

class NativeDarwinRouteConfig {
public:
  explicit NativeDarwinRouteConfig(
      NativeDarwinRouteApi api,
      NativeDarwinRouteConfigOptions options = {});
  ~NativeDarwinRouteConfig() = default;

  NativeDarwinRouteConfig(const NativeDarwinRouteConfig &) = delete;
  NativeDarwinRouteConfig &
  operator=(const NativeDarwinRouteConfig &) = delete;

  NativeDarwinRouteConfigResult
  configure(const vpn_engine::TunnelMetadata &metadata);
  NativeDarwinRouteConfigResult cleanup();

  const std::vector<NativeDarwinRoute> &owned_routes() const;
  int effective_mtu() const;

private:
  NativeDarwinRouteApi api_;
  NativeDarwinRouteConfigOptions options_;
  std::vector<NativeDarwinRoute> owned_routes_;
  int effective_mtu_ = 0;
};

} // namespace platform
} // namespace exv
