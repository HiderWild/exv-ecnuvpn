#include "helper/helper_network_ops.hpp"

#include "platform/common/platform_network_ops.hpp"

#include <map>
#include <memory>
#include <optional>
#include <string>
#include <utility>

namespace exv::helper {
namespace {

platform::RouteEntry to_platform_route(const RouteEntry &route) {
  platform::RouteEntry converted;
  converted.destination = route.destination;
  converted.gateway = route.gateway;
  converted.metric = route.metric;
  converted.is_default = route.destination == "0.0.0.0/0" ||
                         route.destination == "::/0";
  return converted;
}

platform::DnsConfig to_platform_dns(const DnsConfig &dns) {
  platform::DnsConfig converted;
  converted.servers = dns.servers;
  converted.search_domain = dns.search_domain;
  converted.suffixes = dns.suffixes;
  return converted;
}

std::optional<platform::CleanupPolicy>
to_platform_cleanup_policy(const CleanupPolicy &policy) {
  if (policy.remove_adapter)
    return platform::CleanupPolicy::Full;
  if (policy.remove_routes && policy.remove_dns && policy.remove_firewall_rules)
    return platform::CleanupPolicy::KeepAdapter;
  if (policy.remove_routes && !policy.remove_dns &&
      !policy.remove_firewall_rules)
    return platform::CleanupPolicy::RoutesOnly;
  if (!policy.remove_routes && policy.remove_dns &&
      !policy.remove_firewall_rules)
    return platform::CleanupPolicy::DnsOnly;
  return std::nullopt;
}

std::string adapter_from_resources(
    const std::vector<ManagedResource> &resources) {
  for (const auto &resource : resources) {
    if (resource.type == "adapter")
      return resource.detail;
  }
  return {};
}

std::vector<platform::ManagedNetworkResource> to_platform_resources(
    const std::vector<ManagedResource> &resources) {
  std::vector<platform::ManagedNetworkResource> converted;
  converted.reserve(resources.size());
  for (const auto &resource : resources) {
    converted.push_back({resource.type, resource.detail});
  }
  return converted;
}

ManagedResource to_helper_resource(
    const platform::ManagedNetworkResource &resource) {
  return {resource.type, resource.detail};
}

void append_unique_resource(std::vector<ManagedResource> *resources,
                            const ManagedResource &resource) {
  if (!resources)
    return;
  for (const auto &existing : *resources) {
    if (existing.type == resource.type && existing.detail == resource.detail)
      return;
  }
  resources->push_back(resource);
}

bool append_platform_resources(
    platform::PlatformNetworkOps *platform_ops,
    std::vector<ManagedResource> *created_resources) {
  if (!platform_ops || !created_resources)
    return false;
  auto platform_resources = platform_ops->managed_resources();
  if (platform_resources.empty())
    return false;
  for (const auto &resource : platform_resources) {
    append_unique_resource(created_resources, to_helper_resource(resource));
  }
  return true;
}

void register_config_resources(
    const ApplyTunnelConfigRequest &request,
    std::vector<ManagedResource> *created_resources) {
  if (!created_resources)
    return;
  for (const auto &route : request.config.routes) {
    created_resources->push_back({"route", route.destination});
  }
  for (const auto &server : request.config.dns.servers) {
    created_resources->push_back({"dns", server});
  }
}

class PlatformBackedHelperNetworkOps final : public HelperNetworkOps {
public:
  explicit PlatformBackedHelperNetworkOps(
      std::unique_ptr<platform::PlatformNetworkOps> platform_ops)
      : platform_ops_(std::move(platform_ops)) {}

  PrepareTunnelDeviceResponse prepare_tunnel_device(
      const PrepareTunnelDeviceRequest &request,
      std::vector<ManagedResource> *created_resources) override {
    PrepareTunnelDeviceResponse response;
    if (!platform_ops_) {
      response.error_code = "network_ops_unavailable";
      response.error_message = "Platform network operations are not available";
      return response;
    }

    auto descriptor = platform_ops_->prepare_tunnel_device(request.adapter_name);
    if (!descriptor.is_open || descriptor.path.empty()) {
      response.error_code = descriptor.error_code.empty()
                                ? "device_not_found"
                                : descriptor.error_code;
      response.error_message = descriptor.error_message.empty()
                                   ? "Platform network operations did not return a device"
                                   : descriptor.error_message;
      return response;
    }

    response.device_path = descriptor.path;
    response.mtu = descriptor.mtu;
    session_devices_[request.session_id.value] = descriptor;
    if (created_resources &&
        !append_platform_resources(platform_ops_.get(), created_resources)) {
      append_unique_resource(created_resources,
                             {"adapter", descriptor.adapter_name.empty()
                                             ? request.adapter_name
                                             : descriptor.adapter_name});
    }
    return response;
  }

  ApplyTunnelConfigResponse apply_tunnel_config(
      const ApplyTunnelConfigRequest &request,
      std::vector<ManagedResource> *created_resources) override {
    ApplyTunnelConfigResponse response;
    if (!platform_ops_) {
      response.error_code = "network_ops_unavailable";
      response.error_message = "Platform network operations are not available";
      return response;
    }

    auto device_it = session_devices_.find(request.config.session_id.value);
    if (device_it == session_devices_.end() || !device_it->second.is_open) {
      response.error_code = "device_not_prepared";
      response.error_message = "Tunnel device has not been prepared";
      return response;
    }

    platform::TunnelConfig config;
    config.interface_address = request.config.interface_address;
    config.interface_name = device_it->second.adapter_name;
    config.mtu = device_it->second.mtu;
    config.enable_kill_switch = request.config.enable_kill_switch;
    for (const auto &route : request.config.routes) {
      config.routes.push_back(to_platform_route(route));
    }
    config.server_bypass_ips = request.config.server_bypass_ips;
    config.dns = to_platform_dns(request.config.dns);

    response.success =
        platform_ops_->apply_tunnel_config(device_it->second, config);
    if (!response.success) {
      auto platform_error = platform_ops_->last_error();
      response.error_code = platform_error.code.empty()
                                ? "apply_config_failed"
                                : platform_error.code;
      response.error_message = platform_error.message.empty()
                                   ? "Failed to apply platform tunnel config"
                                   : platform_error.message;
      response.error_target = platform_error.target;
      response.system_error = platform_error.system_error;
      return response;
    }

    if (!append_platform_resources(platform_ops_.get(), created_resources)) {
      register_config_resources(request, created_resources);
    }
    return response;
  }

  CleanupResponse cleanup(
      const SessionId &session_id, const CleanupPolicy &policy,
      const std::vector<ManagedResource> &resources) override {
    CleanupResponse response;
    if (!platform_ops_) {
      response.errors.push_back("Platform network operations are not available");
      return response;
    }

    std::string adapter_name = adapter_from_resources(resources);
    auto device_it = session_devices_.find(session_id.value);
    if (adapter_name.empty() && device_it != session_devices_.end()) {
      adapter_name = device_it->second.adapter_name;
    }
    if (adapter_name.empty()) {
      response.errors.push_back("No managed tunnel adapter is registered");
      return response;
    }

    auto platform_policy = to_platform_cleanup_policy(policy);
    if (!platform_policy.has_value()) {
      response.errors.push_back(
          "Cleanup policy cannot be represented without over-cleaning");
      return response;
    }

    platform::CleanupResult result = platform_ops_->cleanup_resources(
        to_platform_resources(resources), *platform_policy);
    response.success = result.success;
    if (!result.success) {
      response.errors.push_back(
          result.error_message.empty() ? "Platform cleanup failed"
                                       : result.error_message);
      return response;
    }

    session_devices_.erase(session_id.value);
    return response;
  }

private:
  std::unique_ptr<platform::PlatformNetworkOps> platform_ops_;
  std::map<std::string, platform::TunnelDeviceDescriptor> session_devices_;
};

} // namespace

std::shared_ptr<HelperNetworkOps> create_helper_network_ops() {
  return create_helper_network_ops(platform::PlatformNetworkOps::create());
}

std::shared_ptr<HelperNetworkOps>
create_helper_network_ops(std::unique_ptr<platform::PlatformNetworkOps> platform_ops) {
  if (!platform_ops)
    return nullptr;
  return std::make_shared<PlatformBackedHelperNetworkOps>(std::move(platform_ops));
}

} // namespace exv::helper
