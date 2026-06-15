#include "platform/win32/platform_network_ops_win32.hpp"

#include "vpn_engine/session_state.hpp"

#include <cstdint>
#include <memory>
#include <optional>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

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

bool has_dns_config(const DnsConfig &dns) {
  return !dns.servers.empty() || !dns.search_domain.empty() ||
         !dns.suffixes.empty();
}

bool is_blank(const std::string &value) {
  for (char ch : value) {
    if (ch != ' ' && ch != '\t' && ch != '\r' && ch != '\n')
      return false;
  }
  return true;
}

bool to_native_dns_settings(const DnsConfig &dns,
                            ecnuvpn::platform::NativeDnsSettings *settings) {
  if (!settings)
    return false;

  for (const auto &server : dns.servers) {
    if (is_blank(server))
      return false;
  }
  for (const auto &suffix : dns.suffixes) {
    if (is_blank(suffix))
      return false;
  }

  settings->servers = dns.servers;
  settings->search_domain = dns.search_domain;
  settings->suffixes = dns.suffixes;
  return true;
}

std::vector<std::string> split(const std::string &value, char delimiter) {
  std::vector<std::string> parts;
  std::size_t start = 0;
  while (start <= value.size()) {
    const std::size_t pos = value.find(delimiter, start);
    if (pos == std::string::npos) {
      parts.push_back(value.substr(start));
      break;
    }
    parts.push_back(value.substr(start, pos - start));
    start = pos + 1;
  }
  return parts;
}

std::string join(const std::vector<std::string> &values, char delimiter) {
  std::string out;
  for (std::size_t i = 0; i < values.size(); ++i) {
    if (i > 0)
      out.push_back(delimiter);
    out += values[i];
  }
  return out;
}

std::string encode_route_resource(
    const ecnuvpn::platform::NativeIpRoute &route) {
  return route.cidr + "|" + route.destination + "|" +
         std::to_string(route.prefix_length) + "|" +
         std::to_string(route.interface_index) + "|" + route.next_hop + "|" +
         (route.server_bypass ? "1" : "0");
}

std::optional<ecnuvpn::platform::NativeIpRoute>
decode_route_resource(const std::string &detail) {
  auto parts = split(detail, '|');
  if (parts.size() != 6)
    return std::nullopt;
  ecnuvpn::platform::NativeIpRoute route;
  route.cidr = parts[0];
  route.destination = parts[1];
  try {
    route.prefix_length = std::stoi(parts[2]);
    route.interface_index = static_cast<std::uint32_t>(std::stoul(parts[3]));
  } catch (...) {
    return std::nullopt;
  }
  route.next_hop = parts[4];
  route.server_bypass = parts[5] == "1";
  if (route.destination.empty() || route.prefix_length < 0 ||
      route.prefix_length > 32 || route.interface_index == 0)
    return std::nullopt;
  return route;
}

std::string encode_dns_resource(
    std::uint32_t interface_index,
    const ecnuvpn::platform::NativeDnsSettings &settings) {
  return std::to_string(interface_index) + "|" + join(settings.servers, ',') +
         "|" + settings.search_domain + "|" + join(settings.suffixes, ',');
}

std::optional<std::pair<std::uint32_t, ecnuvpn::platform::NativeDnsSettings>>
decode_dns_resource(const std::string &detail) {
  auto parts = split(detail, '|');
  if (parts.size() != 4)
    return std::nullopt;
  std::uint32_t interface_index = 0;
  try {
    interface_index = static_cast<std::uint32_t>(std::stoul(parts[0]));
  } catch (...) {
    return std::nullopt;
  }
  ecnuvpn::platform::NativeDnsSettings settings;
  if (!parts[1].empty())
    settings.servers = split(parts[1], ',');
  settings.search_domain = parts[2];
  if (!parts[3].empty())
    settings.suffixes = split(parts[3], ',');
  return std::make_pair(interface_index, settings);
}

std::optional<std::string> first_resource_detail(
    const std::vector<ManagedNetworkResource> &resources,
    const std::string &type) {
  for (const auto &resource : resources) {
    if (resource.type == type && !resource.detail.empty())
      return resource.detail;
  }
  return std::nullopt;
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
    managed_resources_.clear();

    TunnelDeviceDescriptor descriptor;
    descriptor.path = "wintun://" + narrow_ascii(started.metadata.adapter_name);
    descriptor.adapter_name = narrow_ascii(started.metadata.adapter_name);
    descriptor.mtu = mtu > 0 ? mtu : 1400;
    descriptor.is_open = true;
    last_device_ = descriptor;
    managed_resources_.push_back({"adapter", descriptor.adapter_name});
    managed_resources_.push_back(
        {"win32_adapter_if_index", std::to_string(interface_index_)});
    managed_resources_.push_back(
        {"win32_adapter_luid", std::to_string(started.metadata.luid)});
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

    std::string address;
    std::string netmask;
    if (!split_ipv4_cidr(config.interface_address, &address, &netmask))
      return false;

    ecnuvpn::platform::NativeDnsSettings desired_dns;
    const bool configure_dns = has_dns_config(config.dns);
    if (configure_dns) {
      if (!to_native_dns_settings(config.dns, &desired_dns))
        return false;
      if (!ip_helper_api_.get_interface_dns_settings ||
          !ip_helper_api_.set_interface_dns_settings)
        return false;
    }

    ecnuvpn::platform::NativeDnsSettings previous_dns;
    if (configure_dns) {
      ecnuvpn::platform::NativeIpHelperApi::ErrorCode dns_error =
          ip_helper_api_.get_interface_dns_settings(interface_index_,
                                                    previous_dns);
      if (dns_error != 0)
        return false;
    }

    ecnuvpn::vpn_engine::TunnelMetadata metadata;
    metadata.interface_name = device.adapter_name;
    metadata.interface_index = static_cast<int>(interface_index_);
    metadata.internal_ip4_address = address;
    metadata.internal_ip4_netmask = netmask;
    metadata.mtu = config.mtu > 0 ? config.mtu : device.mtu;
    for (const auto &route : config.routes)
      metadata.routes.push_back(route.destination);
    metadata.server_bypass_ips = config.server_bypass_ips;

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

    if (configure_dns) {
      ecnuvpn::platform::NativeIpHelperApi::ErrorCode dns_error =
          ip_helper_api_.set_interface_dns_settings(interface_index_,
                                                    desired_dns);
      if (dns_error != 0) {
        (void)ip_helper_api_.set_interface_dns_settings(interface_index_,
                                                        previous_dns);
        auto rollback = ip_config->cleanup();
        if (!rollback.ok())
          ip_config_ = std::move(ip_config);
        return false;
      }
      original_dns_ = previous_dns;
      dns_configured_ = true;
    }

    ip_config_ = std::move(ip_config);
    for (const auto &route : ip_config_->owned_routes()) {
      managed_resources_.push_back({"win32_route", encode_route_resource(route)});
    }
    if (configure_dns) {
      managed_resources_.push_back(
          {"win32_dns_original",
           encode_dns_resource(interface_index_, previous_dns)});
    }
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
        result.error_message = cleaned.message.empty()
                                   ? "native route cleanup failed"
                                   : cleaned.message;
      } else {
        result.routes_removed = owned_route_count;
        ip_config_.reset();
      }
    }

    if (dns_configured_ && original_dns_ &&
        (policy == CleanupPolicy::Full || policy == CleanupPolicy::KeepAdapter ||
         policy == CleanupPolicy::DnsOnly)) {
      if (!ip_helper_api_.set_interface_dns_settings) {
        result.success = false;
        result.error_message = "native DNS settings API is missing";
        return result;
      }
      ecnuvpn::platform::NativeIpHelperApi::ErrorCode dns_error =
          ip_helper_api_.set_interface_dns_settings(interface_index_,
                                                    *original_dns_);
      if (dns_error != 0) {
        result.success = false;
        if (result.error_message.empty()) {
          result.error_message =
              "SetInterfaceDnsSettings restore failed: " +
              std::to_string(dns_error);
        }
      } else {
        result.dns_removed = true;
        dns_configured_ = false;
        original_dns_.reset();
      }
    }

    if (!result.success)
      return result;

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
      managed_resources_.clear();
      result.adapter_removed = true;
    }

    return result;
  }

  CleanupResult cleanup_resources(
      const std::vector<ManagedNetworkResource> &resources,
      CleanupPolicy policy) override {
    if (ip_config_ || dns_configured_ || wintun_) {
      return cleanup(first_resource_detail(resources, "adapter").value_or({}),
                     policy);
    }

    CleanupResult result;
    result.success = true;

    const bool remove_routes =
        policy == CleanupPolicy::Full || policy == CleanupPolicy::KeepAdapter ||
        policy == CleanupPolicy::RoutesOnly;
    if (remove_routes) {
      for (const auto &resource : resources) {
        if (resource.type != "win32_route")
          continue;
        auto route = decode_route_resource(resource.detail);
        if (!route.has_value())
          continue;
        if (!ip_helper_api_.delete_ip_forward_entry2 ||
            ip_helper_api_.delete_ip_forward_entry2(*route) != 0) {
          result.success = false;
          if (result.error_message.empty())
            result.error_message = "Windows route cleanup from facts failed";
        } else {
          ++result.routes_removed;
        }
      }
    }

    const bool remove_dns =
        policy == CleanupPolicy::Full || policy == CleanupPolicy::KeepAdapter ||
        policy == CleanupPolicy::DnsOnly;
    if (remove_dns) {
      for (const auto &resource : resources) {
        if (resource.type != "win32_dns_original")
          continue;
        auto dns = decode_dns_resource(resource.detail);
        if (!dns.has_value())
          continue;
        if (!ip_helper_api_.set_interface_dns_settings ||
            ip_helper_api_.set_interface_dns_settings(dns->first,
                                                      dns->second) != 0) {
          result.success = false;
          if (result.error_message.empty())
            result.error_message = "Windows DNS restore from facts failed";
        } else {
          result.dns_removed = true;
        }
      }
    }

    if (!result.success)
      return result;

    if (policy == CleanupPolicy::Full) {
      auto adapter = first_resource_detail(resources, "adapter");
      if (adapter.has_value()) {
        ecnuvpn::platform::NativeWintunConfig config;
        config.adapter_name_prefix = widen_ascii(*adapter);
        auto wintun = std::make_unique<ecnuvpn::platform::NativeWintun>(
            wintun_dependencies_, config);
        auto started = wintun->start();
        if (!started.ok()) {
          result.success = false;
          result.error_message = started.detail.empty()
                                     ? "failed to open Wintun adapter for cleanup"
                                     : started.detail;
          return result;
        }
        auto deleted = wintun->delete_adapter();
        if (!deleted.ok()) {
          result.success = false;
          result.error_message = deleted.detail;
          return result;
        }
        result.adapter_removed = true;
      }
    }

    return result;
  }

  std::vector<ManagedNetworkResource> managed_resources() const override {
    return managed_resources_;
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
  std::optional<ecnuvpn::platform::NativeDnsSettings> original_dns_;
  TunnelDeviceDescriptor last_device_;
  std::uint32_t interface_index_ = 0;
  bool dns_configured_ = false;
  std::vector<ManagedNetworkResource> managed_resources_;
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
