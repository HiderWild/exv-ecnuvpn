#include "platform/linux/platform_network_ops_linux.hpp"

#include "platform/common/process_utils.hpp"

#include <fcntl.h>
#include <linux/if.h>
#include <linux/if_tun.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include <algorithm>
#include <cstring>
#include <optional>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

namespace exv::platform {
namespace {

std::string shell_quote(const std::string& value) {
  return ecnuvpn::platform::shell_quote(value);
}

bool run_ok(const std::string& command) {
  return ecnuvpn::platform::run_command(command) == 0;
}

std::string trim_interface_name(std::string name) {
  if (name.empty()) {
    name = "exv%d";
  }
  if (name.size() >= IFNAMSIZ) {
    name.resize(IFNAMSIZ - 1);
  }
  return name;
}

bool has_dns_config(const DnsConfig& dns) {
  return !dns.servers.empty() || !dns.search_domain.empty() ||
         !dns.suffixes.empty();
}

std::string join_servers(const std::vector<std::string>& servers) {
  std::ostringstream out;
  for (std::size_t i = 0; i < servers.size(); ++i) {
    if (i > 0) {
      out << ' ';
    }
    out << shell_quote(servers[i]);
  }
  return out.str();
}

std::vector<std::string> split(const std::string& value, char delimiter) {
  std::vector<std::string> parts;
  std::string part;
  std::istringstream input(value);
  while (std::getline(input, part, delimiter)) {
    parts.push_back(part);
  }
  if (!value.empty() && value.back() == delimiter) {
    parts.emplace_back();
  }
  return parts;
}

void append_unique_resource(std::vector<ManagedNetworkResource>* resources,
                            ManagedNetworkResource resource) {
  if (!resources) {
    return;
  }
  for (const auto& existing : *resources) {
    if (existing.type == resource.type && existing.detail == resource.detail) {
      return;
    }
  }
  resources->push_back(std::move(resource));
}

std::string adapter_from_resources(
    const std::vector<ManagedNetworkResource>& resources) {
  for (const auto& resource : resources) {
    if (resource.type == "adapter") {
      return resource.detail;
    }
  }
  return {};
}

struct LinuxRouteFact {
  std::string destination;
  std::string gateway;
  std::string device;
  int metric = 0;
};

std::string encode_route_fact(const LinuxRouteFact& fact) {
  return fact.destination + "|" + fact.gateway + "|" + fact.device + "|" +
         std::to_string(fact.metric);
}

std::optional<LinuxRouteFact> decode_route_fact(const std::string& detail) {
  auto parts = split(detail, '|');
  if (parts.size() != 4 || parts[0].empty()) {
    return std::nullopt;
  }
  LinuxRouteFact fact;
  fact.destination = parts[0];
  fact.gateway = parts[1];
  fact.device = parts[2];
  try {
    fact.metric = parts[3].empty() ? 0 : std::stoi(parts[3]);
  } catch (...) {
    return std::nullopt;
  }
  return fact;
}

std::string route_delete_command(LinuxRouteFact fact,
                                 const std::string& fallback_device) {
  if (fact.device.empty()) {
    fact.device = fallback_device;
  }
  std::string command = "ip route del " + shell_quote(fact.destination);
  if (!fact.gateway.empty()) {
    command += " via " + shell_quote(fact.gateway);
  }
  if (!fact.device.empty()) {
    command += " dev " + shell_quote(fact.device);
  }
  if (fact.metric > 0) {
    command += " metric " + std::to_string(fact.metric);
  }
  command += " >/dev/null 2>&1";
  return command;
}

struct LinuxDefaultRoute {
  std::string gateway;
  std::string device;
};

std::optional<LinuxDefaultRoute> read_default_route() {
  const std::string output = ecnuvpn::platform::run_command_output(
      "ip route show default 0.0.0.0/0");
  std::istringstream lines(output);
  std::string line;
  while (std::getline(lines, line)) {
    std::istringstream tokens(line);
    std::string token;
    LinuxDefaultRoute route;
    while (tokens >> token) {
      if (token == "via") {
        tokens >> route.gateway;
      } else if (token == "dev") {
        tokens >> route.device;
      }
    }
    if (!route.gateway.empty() && !route.device.empty()) {
      return route;
    }
  }
  return std::nullopt;
}

class LinuxPlatformNetworkOps final : public PlatformNetworkOps {
public:
  LinuxPlatformNetworkOps() = default;
  ~LinuxPlatformNetworkOps() override {
    if (fd_ >= 0) {
      ::close(fd_);
    }
  }

  TunnelDeviceDescriptor prepare_tunnel_device(const std::string& adapter_name,
                                               int mtu = 1400) override {
    const int fd = ::open("/dev/net/tun", O_RDWR | O_CLOEXEC);
    if (fd < 0) {
      return {};
    }

    ifreq ifr{};
    ifr.ifr_flags = IFF_TUN | IFF_NO_PI;
    const std::string requested = trim_interface_name(adapter_name);
    std::strncpy(ifr.ifr_name, requested.c_str(), IFNAMSIZ - 1);

    if (::ioctl(fd, TUNSETIFF, &ifr) < 0) {
      ::close(fd);
      return {};
    }

    const int persist = 1;
    if (::ioctl(fd, TUNSETPERSIST, persist) < 0) {
      ::close(fd);
      return {};
    }

    if (fd_ >= 0) {
      ::close(fd_);
    }
    fd_ = fd;

    TunnelDeviceDescriptor descriptor;
    descriptor.path = std::string("/dev/net/tun:") + ifr.ifr_name;
    descriptor.adapter_name = ifr.ifr_name;
    descriptor.fd = fd;
    descriptor.mtu = mtu > 0 ? mtu : 1400;
    descriptor.is_open = true;
    last_device_ = descriptor;
    managed_resources_.clear();
    append_unique_resource(&managed_resources_,
                           {"adapter", descriptor.adapter_name});
    return descriptor;
  }

  TunnelDeviceDescriptor open_tunnel_device(
      const std::string& adapter_name) override {
    const std::string requested = trim_interface_name(adapter_name);
    const int fd = ::open("/dev/net/tun", O_RDWR | O_CLOEXEC);
    if (fd < 0) {
      return {};
    }

    ifreq ifr{};
    ifr.ifr_flags = IFF_TUN | IFF_NO_PI;
    std::strncpy(ifr.ifr_name, requested.c_str(), IFNAMSIZ - 1);
    if (::ioctl(fd, TUNSETIFF, &ifr) < 0) {
      ::close(fd);
      return {};
    }

    if (fd_ >= 0) {
      ::close(fd_);
    }
    fd_ = fd;

    TunnelDeviceDescriptor descriptor;
    descriptor.path = std::string("/dev/net/tun:") + ifr.ifr_name;
    descriptor.adapter_name = ifr.ifr_name;
    descriptor.fd = fd;
    descriptor.mtu = last_device_.mtu > 0 ? last_device_.mtu : 1400;
    descriptor.is_open = true;
    last_device_ = descriptor;
    managed_resources_.clear();
    append_unique_resource(&managed_resources_,
                           {"adapter", descriptor.adapter_name});
    return descriptor;
  }

  bool apply_tunnel_config(const TunnelDeviceDescriptor& device,
                           const TunnelConfig& config) override {
    if (!device.is_open || device.adapter_name.empty()) {
      return false;
    }

    std::vector<ManagedNetworkResource> applied_resources;
    const std::string dev = shell_quote(device.adapter_name);
    const int mtu = config.mtu > 0 ? config.mtu : device.mtu;
    if (!run_ok("ip link set dev " + dev + " mtu " + std::to_string(mtu))) {
      return false;
    }
    if (!run_ok("ip addr flush dev " + dev)) {
      return false;
    }
    if (!config.interface_address.empty() &&
        !run_ok("ip addr add " + shell_quote(config.interface_address) +
                " dev " + dev)) {
      return false;
    }
    if (!run_ok("ip link set dev " + dev + " up")) {
      return false;
    }
    append_unique_resource(&applied_resources,
                           {"linux_interface_config",
                            device.adapter_name + "|" +
                                config.interface_address + "|" +
                                std::to_string(mtu)});

    for (const auto& route : config.routes) {
      std::string command = "ip route replace " +
                            shell_quote(route.destination);
      if (!route.gateway.empty()) {
        command += " via " + shell_quote(route.gateway);
      }
      command += " dev " + dev;
      if (route.metric > 0) {
        command += " metric " + std::to_string(route.metric);
      }
      if (!run_ok(command)) {
        return false;
      }
      append_unique_resource(
          &applied_resources,
          {"linux_route",
           encode_route_fact({route.destination, route.gateway,
                              device.adapter_name, route.metric})});
    }

    for (const auto& bypass : config.server_bypass_ips) {
      auto default_route = read_default_route();
      if (!default_route.has_value()) {
        return false;
      }
      const std::string command =
          "ip route replace " + shell_quote(bypass) + " via " +
          shell_quote(default_route->gateway) + " dev " +
          shell_quote(default_route->device);
      if (!run_ok(command)) {
        return false;
      }
      append_unique_resource(
          &applied_resources,
          {"linux_server_bypass_route",
           encode_route_fact({bypass, default_route->gateway,
                              default_route->device, 0})});
    }

    if (has_dns_config(config.dns)) {
      if (config.dns.servers.empty()) {
        return false;
      }
      if (!run_ok("resolvectl dns " + dev + " " +
                  join_servers(config.dns.servers))) {
        return false;
      }
      if (!config.dns.search_domain.empty() &&
          !run_ok("resolvectl domain " + dev + " " +
                  shell_quote(config.dns.search_domain))) {
        return false;
      }
      append_unique_resource(&applied_resources,
                             {"linux_dns", device.adapter_name});
    }

    for (const auto& resource : applied_resources) {
      append_unique_resource(&managed_resources_, resource);
    }
    last_device_ = device;
    return true;
  }

  CleanupResult cleanup(const std::string& adapter_name,
                        CleanupPolicy policy) override {
    std::vector<ManagedNetworkResource> resources;
    if (!adapter_name.empty()) {
      resources.push_back({"adapter", adapter_name});
    }
    const auto current_resources = managed_resources();
    resources.insert(resources.end(), current_resources.begin(),
                     current_resources.end());
    return cleanup_resources(resources, policy);
  }

  CleanupResult cleanup_resources(
      const std::vector<ManagedNetworkResource>& resources,
      CleanupPolicy policy) override {
    CleanupResult result;
    result.success = true;

    const std::string adapter_name =
        adapter_from_resources(resources).empty()
            ? last_device_.adapter_name
            : adapter_from_resources(resources);
    const std::string dev =
        adapter_name.empty() ? std::string() : shell_quote(adapter_name);

    const bool remove_routes = policy == CleanupPolicy::Full ||
                               policy == CleanupPolicy::KeepAdapter ||
                               policy == CleanupPolicy::RoutesOnly;
    if (remove_routes) {
      for (const auto& resource : resources) {
        if (resource.type == "linux_route" ||
            resource.type == "linux_server_bypass_route") {
          auto fact = decode_route_fact(resource.detail);
          if (!fact.has_value()) {
            result.success = false;
            if (result.error_message.empty()) {
              result.error_message = "invalid Linux route cleanup fact";
            }
            continue;
          }
          if (!run_ok(route_delete_command(*fact, adapter_name))) {
            result.success = false;
            if (result.error_message.empty()) {
              result.error_message = "Linux route cleanup from facts failed";
            }
          } else {
            ++result.routes_removed;
          }
        }
      }
    }

    const bool remove_dns = policy == CleanupPolicy::Full ||
                            policy == CleanupPolicy::KeepAdapter ||
                            policy == CleanupPolicy::DnsOnly;
    if (remove_dns) {
      bool saw_dns_fact = false;
      for (const auto& resource : resources) {
        if (resource.type == "linux_dns") {
          saw_dns_fact = true;
          const std::string dns_dev = resource.detail.empty()
                                          ? dev
                                          : shell_quote(resource.detail);
          if (dns_dev.empty()) {
            continue;
          }
          if (run_ok("resolvectl revert " + dns_dev + " >/dev/null 2>&1")) {
            result.dns_removed = true;
          } else {
            result.success = false;
            if (result.error_message.empty()) {
              result.error_message = "resolvectl revert failed";
            }
          }
        }
      }
      if (!saw_dns_fact && !dev.empty()) {
        result.dns_removed =
            run_ok("resolvectl revert " + dev + " >/dev/null 2>&1");
        if (!result.dns_removed && policy == CleanupPolicy::DnsOnly) {
          result.success = false;
          result.error_message = "resolvectl revert failed";
          return result;
        }
      }
    }

    if (!result.success) {
      return result;
    }

    if (policy == CleanupPolicy::Full && !dev.empty()) {
      (void)run_ok("ip link set dev " + dev + " down >/dev/null 2>&1");
      if (fd_ >= 0) {
        const int persist = 0;
        (void)::ioctl(fd_, TUNSETPERSIST, persist);
        ::close(fd_);
        fd_ = -1;
      }
      result.adapter_removed =
          run_ok("ip tuntap del dev " + dev + " mode tun >/dev/null 2>&1");
      if (!result.adapter_removed) {
        result.success = false;
        result.error_message = "ip tuntap delete failed";
        return result;
      }
      last_device_ = {};
      managed_resources_.clear();
    }

    return result;
  }

  std::vector<ManagedNetworkResource> managed_resources() const override {
    return managed_resources_;
  }

  bool device_exists(const std::string& adapter_name) const override {
    if (adapter_name.empty()) {
      return last_device_.is_open;
    }
    return run_ok("ip link show dev " + shell_quote(adapter_name) +
                  " >/dev/null 2>&1");
  }

private:
  int fd_ = -1;
  TunnelDeviceDescriptor last_device_;
  std::vector<ManagedNetworkResource> managed_resources_;
};

} // namespace

std::unique_ptr<PlatformNetworkOps> create_linux_platform_network_ops() {
  return std::make_unique<LinuxPlatformNetworkOps>();
}

} // namespace exv::platform
