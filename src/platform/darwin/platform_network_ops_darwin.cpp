#include "platform/darwin/platform_network_ops_darwin.hpp"

#include "vpn_engine/session_state.hpp"

#include <cstdint>
#include <memory>
#include <sstream>
#include <string>
#include <utility>

namespace exv::platform {
namespace {

bool parse_prefix(const std::string &text, int *prefix) {
  if (text.empty() || !prefix)
    return false;
  int parsed = 0;
  for (char ch : text) {
    if (ch < '0' || ch > '9')
      return false;
    parsed = parsed * 10 + (ch - '0');
    if (parsed > 32)
      return false;
  }
  *prefix = parsed;
  return true;
}

std::string netmask_from_prefix(int prefix) {
  if (prefix < 0 || prefix > 32)
    return {};
  const std::uint32_t mask =
      prefix == 0 ? 0U : (0xffffffffU << (32 - prefix));
  std::ostringstream out;
  out << ((mask >> 24) & 0xffU) << '.' << ((mask >> 16) & 0xffU) << '.'
      << ((mask >> 8) & 0xffU) << '.' << (mask & 0xffU);
  return out.str();
}

bool split_ipv4_cidr(const std::string &cidr, std::string *address,
                     std::string *netmask) {
  const std::size_t slash = cidr.find('/');
  if (slash == std::string::npos || slash == 0 || slash + 1 >= cidr.size())
    return false;
  if (cidr.find('/', slash + 1) != std::string::npos)
    return false;

  int prefix = 0;
  if (!parse_prefix(cidr.substr(slash + 1), &prefix))
    return false;

  const std::string parsed_address = cidr.substr(0, slash);
  const std::string parsed_netmask = netmask_from_prefix(prefix);
  if (parsed_address.empty() || parsed_netmask.empty())
    return false;

  *address = parsed_address;
  *netmask = parsed_netmask;
  return true;
}

class DarwinPlatformNetworkOps final : public PlatformNetworkOps {
public:
  DarwinPlatformNetworkOps(ecnuvpn::platform::NativeUtunApi utun_api,
                           ecnuvpn::platform::NativeDarwinRouteApi route_api)
      : utun_api_(std::move(utun_api)), route_api_(std::move(route_api)) {}

  TunnelDeviceDescriptor prepare_tunnel_device(const std::string &adapter_name,
                                               int mtu = 1400) override {
    (void)adapter_name;
    ecnuvpn::platform::NativeUtunConfig config;
    config.mtu = mtu > 0 ? mtu : 1400;

    auto utun =
        std::make_unique<ecnuvpn::platform::NativeUtun>(utun_api_, config);
    auto started = utun->start();
    if (!started.ok())
      return {};

    utun_ = std::move(utun);

    TunnelDeviceDescriptor descriptor;
    descriptor.path = "utun://" + started.metadata.interface_name;
    descriptor.adapter_name = started.metadata.interface_name;
    descriptor.fd = started.metadata.fd;
    descriptor.mtu = started.metadata.mtu > 0 ? started.metadata.mtu : config.mtu;
    descriptor.is_open = true;
    last_device_ = descriptor;
    return descriptor;
  }

  TunnelDeviceDescriptor
  open_tunnel_device(const std::string &adapter_name) override {
    if (last_device_.is_open &&
        (adapter_name.empty() || adapter_name == last_device_.adapter_name))
      return last_device_;
    return {};
  }

  bool apply_tunnel_config(const TunnelDeviceDescriptor &device,
                           const TunnelConfig &config) override {
    if (!utun_ || !utun_->running() || !device.is_open ||
        device.adapter_name.empty())
      return false;
    if (!config.dns.servers.empty() || !config.dns.search_domain.empty() ||
        !config.dns.suffixes.empty())
      return false;

    if (route_config_) {
      auto cleaned = route_config_->cleanup();
      if (!cleaned.ok())
        return false;
      route_config_.reset();
    }

    std::string address;
    std::string netmask;
    if (!split_ipv4_cidr(config.interface_address, &address, &netmask))
      return false;

    ecnuvpn::vpn_engine::TunnelMetadata metadata;
    metadata.interface_name = device.adapter_name;
    metadata.interface_index = 0;
    metadata.internal_ip4_address = address;
    metadata.internal_ip4_netmask = netmask;
    metadata.mtu = config.mtu > 0 ? config.mtu : device.mtu;
    for (const auto &route : config.routes)
      metadata.routes.push_back(route.destination);
    metadata.server_bypass_ips = config.server_bypass_ips;

    ecnuvpn::platform::NativeDarwinRouteConfigOptions options;
    options.interface_name = device.adapter_name;
    options.configured_mtu = metadata.mtu;

    auto route_config =
        std::make_unique<ecnuvpn::platform::NativeDarwinRouteConfig>(
            route_api_, options);
    auto configured = route_config->configure(metadata);
    if (!configured.ok()) {
      auto rollback = route_config->cleanup();
      if (!rollback.ok())
        route_config_ = std::move(route_config);
      return false;
    }

    route_config_ = std::move(route_config);
    return true;
  }

  CleanupResult cleanup(const std::string &adapter_name,
                        CleanupPolicy policy) override {
    CleanupResult result;
    result.success = true;

    const bool should_cleanup_routes =
        policy == CleanupPolicy::Full || policy == CleanupPolicy::KeepAdapter ||
        policy == CleanupPolicy::RoutesOnly;
    if (route_config_ && should_cleanup_routes) {
      const int owned_route_count =
          static_cast<int>(route_config_->owned_routes().size());
      auto cleaned = route_config_->cleanup();
      if (!cleaned.ok()) {
        result.success = false;
        result.error_message = cleaned.message;
        return result;
      }
      result.routes_removed = owned_route_count;
      route_config_.reset();
    }

    if (policy == CleanupPolicy::DnsOnly) {
      result.success = false;
      result.error_message = "Darwin DNS cleanup is not implemented";
      return result;
    }

    if (policy == CleanupPolicy::Full && utun_) {
      (void)adapter_name;
      utun_->stop();
      utun_.reset();
      last_device_ = {};
      result.adapter_removed = true;
    }

    return result;
  }

  bool device_exists(const std::string &adapter_name) const override {
    return utun_ && utun_->running() && last_device_.is_open &&
           (adapter_name.empty() || adapter_name == last_device_.adapter_name);
  }

private:
  ecnuvpn::platform::NativeUtunApi utun_api_;
  ecnuvpn::platform::NativeDarwinRouteApi route_api_;
  std::unique_ptr<ecnuvpn::platform::NativeUtun> utun_;
  std::unique_ptr<ecnuvpn::platform::NativeDarwinRouteConfig> route_config_;
  TunnelDeviceDescriptor last_device_;
};

} // namespace

std::unique_ptr<PlatformNetworkOps> create_darwin_platform_network_ops() {
  return create_darwin_platform_network_ops(
      ecnuvpn::platform::default_native_utun_api(),
      ecnuvpn::platform::default_native_darwin_route_api());
}

std::unique_ptr<PlatformNetworkOps> create_darwin_platform_network_ops(
    ecnuvpn::platform::NativeUtunApi utun_api,
    ecnuvpn::platform::NativeDarwinRouteApi route_api) {
  return std::make_unique<DarwinPlatformNetworkOps>(std::move(utun_api),
                                                   std::move(route_api));
}

} // namespace exv::platform
