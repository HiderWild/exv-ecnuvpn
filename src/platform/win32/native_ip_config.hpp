#pragma once

#include "vpn_engine/engine.hpp"
#include "vpn_engine/session_state.hpp"

#include <cstdint>
#include <functional>
#include <string>
#include <vector>

namespace exv {
namespace platform {

enum class NativeIpConfigError {
  none,
  api_missing,
  invalid_interface,
  invalid_address,
  invalid_mtu,
  invalid_route,
  address_create_failed,
  mtu_set_failed,
  best_route_failed,
  route_create_failed,
  route_delete_failed,
};

const char *native_ip_config_error_code(NativeIpConfigError error);

struct NativeIpConfigResult {
  NativeIpConfigError error = NativeIpConfigError::none;
  std::string message;
  std::string target;
  std::uint32_t system_error = 0;
  int effective_mtu = 0;

  bool ok() const { return error == NativeIpConfigError::none; }
};

struct NativeUnicastAddress {
  std::uint32_t interface_index = 0;
  std::string address;
  int prefix_length = 32;
  std::uint32_t valid_lifetime = 0xFFFFFFFFu;
  std::uint32_t preferred_lifetime = 0xFFFFFFFFu;
  bool skip_as_source = false;
  bool dad_state_preferred = true;
};

struct NativeBestRoute {
  std::uint32_t interface_index = 0;
  std::string next_hop;
};

struct NativeIpRoute {
  std::string cidr;
  std::string destination;
  int prefix_length = 32;
  std::uint32_t interface_index = 0;
  std::string next_hop;
  bool server_bypass = false;
};

struct NativeDnsSettings {
  std::vector<std::string> servers;
  std::string search_domain;
  std::vector<std::string> suffixes;
};

struct NativeIpHelperApi {
  using ErrorCode = std::uint32_t;

  std::function<void(NativeUnicastAddress &)>
      initialize_unicast_ip_address_entry;
  std::function<ErrorCode(const NativeUnicastAddress &)>
      create_unicast_ip_address_entry;
  std::function<ErrorCode(std::uint32_t, int)> set_interface_mtu;
  std::function<ErrorCode(const std::string &, NativeBestRoute &)>
      get_best_route2;
  std::function<ErrorCode(const NativeIpRoute &)> create_ip_forward_entry2;
  std::function<ErrorCode(const NativeIpRoute &)> delete_ip_forward_entry2;
  std::function<ErrorCode(std::uint32_t, NativeDnsSettings &)>
      get_interface_dns_settings;
  std::function<ErrorCode(std::uint32_t, const NativeDnsSettings &)>
      set_interface_dns_settings;
};

NativeIpHelperApi default_native_ip_helper_api();

struct NativeIpConfigOptions {
  std::uint32_t interface_index = 0;
  int configured_mtu = 1290;
  bool configure_address = true;
};

class NativeIpConfig {
public:
  explicit NativeIpConfig(NativeIpHelperApi api,
                          NativeIpConfigOptions options = {});
  ~NativeIpConfig() = default;

  NativeIpConfig(const NativeIpConfig &) = delete;
  NativeIpConfig &operator=(const NativeIpConfig &) = delete;

  NativeIpConfigResult
  configure(const vpn_engine::TunnelMetadata &metadata);
  NativeIpConfigResult cleanup();

  const std::vector<NativeIpRoute> &owned_routes() const;
  int effective_mtu() const;

private:
  NativeIpHelperApi api_;
  NativeIpConfigOptions options_;
  std::vector<NativeIpRoute> owned_routes_;
  int effective_mtu_ = 0;
};

} // namespace platform
} // namespace exv
