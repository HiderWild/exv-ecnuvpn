#include "platform/win32/platform_network_ops_win32.hpp"

#include "vpn_engine/session_state.hpp"

#include <cstdint>
#include <memory>
#include <sstream>
#include <string>
#include <utility>

namespace exv::platform {
namespace {

std::wstring widen_ascii(const std::string &value) {
  std::wstring out;
  out.reserve(value.size());
  for (unsigned char ch : value)
    out.push_back(static_cast<wchar_t>(ch));
  return out;
}

std::string narrow_ascii(const std::wstring &value) {
  std::string out;
  out.reserve(value.size());
  for (wchar_t ch : value)
    out.push_back(ch >= 0 && ch <= 0x7f ? static_cast<char>(ch) : '?');
  return out;
}

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
  out << ((mask >> 24) & 0xffU) << '.'
      << ((mask >> 16) & 0xffU) << '.'
      << ((mask >> 8) & 0xffU) << '.'
      << (mask & 0xffU);
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

class Win32PlatformNetworkOps final : public PlatformNetworkOps {
public:
  Win32PlatformNetworkOps(
      ecnuvpn::platform::NativeWintunDependencies wintun_dependencies,
      ecnuvpn::platform::NativeIpHelperApi ip_helper_api)
      : wintun_dependencies_(std::move(wintun_dependencies)),
        ip_helper_api_(std::move(ip_helper_api)) {}

  TunnelDeviceDescriptor prepare_tunnel_device(const std::string &adapter_name,
                                               int mtu = 1400) override {
    ecnuvpn::platform::NativeWintunConfig config;
    config.adapter_name_prefix =
        widen_ascii(adapter_name.empty() ? "ECNU-VPN" : adapter_name);

    auto wintun = std::make_unique<ecnuvpn::platform::NativeWintun>(
        wintun_dependencies_, config);
    auto started = wintun->start();
    if (!started.ok())
      return {};

    interface_index_ = started.metadata.if_index;
    wintun_ = std::move(wintun);

    TunnelDeviceDescriptor descriptor;
    descriptor.path = "wintun://" + narrow_ascii(started.metadata.adapter_name);
    descriptor.adapter_name = narrow_ascii(started.metadata.adapter_name);
    descriptor.mtu = mtu > 0 ? mtu : 1400;
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
    if (!wintun_ || !device.is_open || interface_index_ == 0)
      return false;
    if (!config.dns.servers.empty() || !config.dns.search_domain.empty())
      return false;

    std::string address;
    std::string netmask;
    if (!split_ipv4_cidr(config.interface_address, &address, &netmask))
      return false;

    ecnuvpn::vpn_engine::TunnelMetadata metadata;
    metadata.interface_name = device.adapter_name;
    metadata.interface_index = static_cast<int>(interface_index_);
    metadata.internal_ip4_address = address;
    metadata.internal_ip4_netmask = netmask;
    metadata.mtu = config.mtu > 0 ? config.mtu : device.mtu;
    for (const auto &route : config.routes)
      metadata.routes.push_back(route.destination);

    ecnuvpn::platform::NativeIpConfigOptions options;
    options.interface_index = interface_index_;
    options.configured_mtu = metadata.mtu;

    auto ip_config = std::make_unique<ecnuvpn::platform::NativeIpConfig>(
        ip_helper_api_, options);
    auto result = ip_config->configure(metadata);
    if (!result.ok()) {
      auto rollback = ip_config->cleanup();
      if (!rollback.ok())
        ip_config_ = std::move(ip_config);
      return false;
    }

    ip_config_ = std::move(ip_config);
    return true;
  }

  CleanupResult cleanup(const std::string &adapter_name,
                        CleanupPolicy policy) override {
    CleanupResult result;
    result.success = true;

    if (ip_config_ && (policy == CleanupPolicy::Full ||
                       policy == CleanupPolicy::KeepAdapter ||
                       policy == CleanupPolicy::RoutesOnly)) {
      const int owned_route_count =
          static_cast<int>(ip_config_->owned_routes().size());
      auto cleaned = ip_config_->cleanup();
      if (!cleaned.ok()) {
        result.success = false;
        result.error_message = cleaned.message;
        return result;
      }
      result.routes_removed = owned_route_count;
      ip_config_.reset();
    }

    if (policy == CleanupPolicy::Full && wintun_) {
      (void)adapter_name;
      auto deleted = wintun_->delete_adapter();
      if (!deleted.ok()) {
        result.success = false;
        result.error_message = deleted.detail;
        return result;
      }
      wintun_.reset();
      interface_index_ = 0;
      last_device_ = {};
      result.adapter_removed = true;
    }

    return result;
  }

  bool device_exists(const std::string &adapter_name) const override {
    return wintun_ && wintun_->running() && last_device_.is_open &&
           (adapter_name.empty() || adapter_name == last_device_.adapter_name);
  }

private:
  ecnuvpn::platform::NativeWintunDependencies wintun_dependencies_;
  ecnuvpn::platform::NativeIpHelperApi ip_helper_api_;
  std::unique_ptr<ecnuvpn::platform::NativeWintun> wintun_;
  std::unique_ptr<ecnuvpn::platform::NativeIpConfig> ip_config_;
  TunnelDeviceDescriptor last_device_;
  std::uint32_t interface_index_ = 0;
};

} // namespace

std::unique_ptr<PlatformNetworkOps> create_win32_platform_network_ops() {
  return create_win32_platform_network_ops(
      ecnuvpn::platform::default_native_wintun_dependencies(),
      ecnuvpn::platform::default_native_ip_helper_api());
}

std::unique_ptr<PlatformNetworkOps> create_win32_platform_network_ops(
    ecnuvpn::platform::NativeWintunDependencies wintun_dependencies,
    ecnuvpn::platform::NativeIpHelperApi ip_helper_api) {
  return std::make_unique<Win32PlatformNetworkOps>(
      std::move(wintun_dependencies), std::move(ip_helper_api));
}

} // namespace exv::platform
