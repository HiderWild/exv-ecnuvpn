#include "platform/darwin/platform_network_ops_darwin.hpp"

#include "platform/common/process_utils.hpp"
#include "vpn_engine/session_state.hpp"

#include <cstdint>
#include <cstdlib>
#include <map>
#include <memory>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

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

bool has_dns_config(const DnsConfig &dns) {
  return !dns.servers.empty() || !dns.search_domain.empty() ||
         !dns.suffixes.empty();
}

NativeDarwinDnsSettings to_native_dns_settings(const DnsConfig &dns) {
  NativeDarwinDnsSettings settings;
  settings.servers = dns.servers;
  settings.search_domain = dns.search_domain;
  settings.suffixes = dns.suffixes;
  return settings;
}

std::string escape_fact_value(const std::string &value) {
  std::string escaped;
  escaped.reserve(value.size());
  for (char ch : value) {
    if (ch == '\\' || ch == '|' || ch == '=') {
      escaped.push_back('\\');
    }
    escaped.push_back(ch);
  }
  return escaped;
}

std::string join_fact_values(const std::vector<std::string> &values) {
  std::string out;
  for (std::size_t i = 0; i < values.size(); ++i) {
    if (i > 0) {
      out.push_back(',');
    }
    out += escape_fact_value(values[i]);
  }
  return out;
}

std::string fact_field(const std::string &key, const std::string &value) {
  return key + "=" + escape_fact_value(value);
}

std::map<std::string, std::string> parse_fact_detail(const std::string &detail) {
  std::map<std::string, std::string> fields;
  std::string key;
  std::string value;
  bool reading_key = true;
  bool escaped = false;

  auto flush = [&] {
    if (!key.empty()) {
      fields[key] = value;
    }
    key.clear();
    value.clear();
    reading_key = true;
  };

  for (char ch : detail) {
    if (escaped) {
      (reading_key ? key : value).push_back(ch);
      escaped = false;
      continue;
    }
    if (ch == '\\') {
      escaped = true;
      continue;
    }
    if (reading_key && ch == '=') {
      reading_key = false;
      continue;
    }
    if (!reading_key && ch == '|') {
      flush();
      continue;
    }
    (reading_key ? key : value).push_back(ch);
  }
  flush();
  return fields;
}

std::vector<std::string> split_fact_values(const std::string &value) {
  std::vector<std::string> values;
  std::string current;
  bool escaped = false;
  for (char ch : value) {
    if (escaped) {
      current.push_back(ch);
      escaped = false;
      continue;
    }
    if (ch == '\\') {
      escaped = true;
      continue;
    }
    if (ch == ',') {
      values.push_back(current);
      current.clear();
      continue;
    }
    current.push_back(ch);
  }
  if (!current.empty() || !value.empty()) {
    values.push_back(current);
  }
  return values;
}

std::string route_fact_detail(const exv::platform::NativeDarwinRoute &route) {
  std::vector<std::string> fields;
  fields.push_back(fact_field("cidr", route.cidr));
  fields.push_back(fact_field("destination", route.destination));
  fields.push_back(fact_field("netmask", route.netmask));
  fields.push_back(fact_field("prefix", std::to_string(route.prefix_length)));
  fields.push_back(fact_field("interface", route.interface_name));
  fields.push_back(fact_field("if_index", std::to_string(route.interface_index)));
  fields.push_back(
      fact_field("message_if_index",
                 std::to_string(route.message_interface_index)));
  fields.push_back(fact_field("message_scoped",
                              route.message_interface_scoped ? "1" : "0"));
  fields.push_back(fact_field("gateway", route.gateway));
  fields.push_back(fact_field("server_bypass", route.server_bypass ? "1" : "0"));

  std::string detail;
  for (std::size_t i = 0; i < fields.size(); ++i) {
    if (i > 0) {
      detail.push_back('|');
    }
    detail += fields[i];
  }
  return detail;
}

bool parse_u32(const std::string &value, std::uint32_t *out) {
  if (!out || value.empty()) {
    return false;
  }
  std::uint64_t parsed = 0;
  for (char ch : value) {
    if (ch < '0' || ch > '9') {
      return false;
    }
    parsed = parsed * 10 + static_cast<unsigned int>(ch - '0');
    if (parsed > UINT32_MAX) {
      return false;
    }
  }
  *out = static_cast<std::uint32_t>(parsed);
  return true;
}

bool parse_int_field(const std::map<std::string, std::string> &fields,
                     const std::string &key, int *out) {
  auto it = fields.find(key);
  if (it == fields.end() || it->second.empty() || !out) {
    return false;
  }
  int parsed = 0;
  for (char ch : it->second) {
    if (ch < '0' || ch > '9') {
      return false;
    }
    parsed = parsed * 10 + (ch - '0');
  }
  *out = parsed;
  return true;
}

bool parse_route_fact(const std::string &detail,
                      exv::platform::NativeDarwinRoute *route) {
  if (!route) {
    return false;
  }
  const auto fields = parse_fact_detail(detail);
  auto get = [&](const std::string &key) -> std::string {
    auto it = fields.find(key);
    return it == fields.end() ? std::string() : it->second;
  };

  exv::platform::NativeDarwinRoute parsed;
  parsed.cidr = get("cidr");
  parsed.destination = get("destination");
  parsed.netmask = get("netmask");
  parsed.interface_name = get("interface");
  parsed.gateway = get("gateway");
  int prefix = 0;
  if (!parse_int_field(fields, "prefix", &prefix)) {
    return false;
  }
  parsed.prefix_length = prefix;
  if (!parse_u32(get("if_index"), &parsed.interface_index)) {
    return false;
  }
  if (!parse_u32(get("message_if_index"), &parsed.message_interface_index)) {
    return false;
  }
  parsed.message_interface_scoped = get("message_scoped") == "1";
  parsed.server_bypass = get("server_bypass") == "1";
  if (parsed.destination.empty() || parsed.netmask.empty() ||
      parsed.interface_name.empty()) {
    return false;
  }
  *route = parsed;
  return true;
}

std::string dns_fact_detail(const std::string &interface_name,
                            const NativeDarwinDnsSettings &settings) {
  std::vector<std::string> fields;
  fields.push_back(fact_field("interface", interface_name));
  fields.push_back("servers=" + join_fact_values(settings.servers));
  fields.push_back(fact_field("search_domain", settings.search_domain));
  fields.push_back("suffixes=" + join_fact_values(settings.suffixes));
  std::string detail;
  for (std::size_t i = 0; i < fields.size(); ++i) {
    if (i > 0) {
      detail.push_back('|');
    }
    detail += fields[i];
  }
  return detail;
}

std::string interface_from_fact(const std::string &detail) {
  const auto fields = parse_fact_detail(detail);
  auto it = fields.find("interface");
  return it == fields.end() ? std::string() : it->second;
}

std::string scutil_array(const std::vector<std::string> &values) {
  std::string out = "*";
  for (const auto &value : values) {
    out.push_back(' ');
    out += value;
  }
  return out;
}

std::string scutil_dns_script(const std::string &interface_name,
                              const NativeDarwinDnsSettings &settings) {
  std::ostringstream script;
  script << "d.init\n";
  script << "d.add InterfaceName " << interface_name << "\n";
  if (!settings.servers.empty()) {
    script << "d.add ServerAddresses " << scutil_array(settings.servers)
           << "\n";
  }
  std::vector<std::string> domains = settings.suffixes;
  if (!settings.search_domain.empty()) {
    domains.insert(domains.begin(), settings.search_domain);
  }
  if (!domains.empty()) {
    script << "d.add SearchDomains " << scutil_array(domains) << "\n";
    script << "d.add SupplementalMatchDomains " << scutil_array(domains)
           << "\n";
  }
  script << "set State:/Network/Service/exv-" << interface_name << "/DNS\n";
  return script.str();
}

class DarwinPlatformNetworkOps final : public PlatformNetworkOps {
public:
  DarwinPlatformNetworkOps(exv::platform::NativeUtunApi utun_api,
                           exv::platform::NativeDarwinRouteApi route_api,
                           NativeDarwinDnsApi dns_api)
      : utun_api_(std::move(utun_api)), route_api_(std::move(route_api)),
        dns_api_(std::move(dns_api)) {}

  TunnelDeviceDescriptor prepare_tunnel_device(const std::string &adapter_name,
                                               int mtu = 1400) override {
    (void)adapter_name;
    exv::platform::NativeUtunConfig config;
    config.mtu = mtu > 0 ? mtu : 1400;

    auto utun =
        std::make_unique<exv::platform::NativeUtun>(utun_api_, config);
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
    remember_adapter(descriptor.adapter_name);
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

    exv::vpn_engine::TunnelMetadata metadata;
    metadata.interface_name = device.adapter_name;
    metadata.interface_index = 0;
    metadata.internal_ip4_address = address;
    metadata.internal_ip4_netmask = netmask;
    metadata.mtu = config.mtu > 0 ? config.mtu : device.mtu;
    for (const auto &route : config.routes)
      metadata.routes.push_back(route.destination);
    metadata.server_bypass_ips = config.server_bypass_ips;

    exv::platform::NativeDarwinRouteConfigOptions options;
    options.interface_name = device.adapter_name;
    options.configured_mtu = metadata.mtu;

    auto route_config =
        std::make_unique<exv::platform::NativeDarwinRouteConfig>(
            route_api_, options);
    auto configured = route_config->configure(metadata);
    if (!configured.ok()) {
      auto rollback = route_config->cleanup();
      if (!rollback.ok())
        route_config_ = std::move(route_config);
      return false;
    }

    route_config_ = std::move(route_config);
    remember_adapter(device.adapter_name);
    remember_routes(route_config_->owned_routes());

    if (has_dns_config(config.dns)) {
      if (!dns_api_.apply_dns || !dns_api_.restore_dns ||
          config.dns.servers.empty()) {
        auto rollback = route_config_->cleanup();
        if (!rollback.ok())
          route_config_.reset();
        return false;
      }
      const NativeDarwinDnsSettings settings =
          to_native_dns_settings(config.dns);
      if (dns_api_.apply_dns(device.adapter_name, settings) != 0) {
        (void)dns_api_.restore_dns(device.adapter_name);
        auto rollback = route_config_->cleanup();
        if (!rollback.ok())
          route_config_.reset();
        forget_dns(device.adapter_name);
        return false;
      }
      remember_dns(device.adapter_name, settings);
    }

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
      forget_routes();
    }

    const bool should_cleanup_dns =
        policy == CleanupPolicy::Full || policy == CleanupPolicy::KeepAdapter ||
        policy == CleanupPolicy::DnsOnly;
    if (should_cleanup_dns && dns_api_.restore_dns &&
        !last_device_.adapter_name.empty() && has_dns_fact(last_device_.adapter_name)) {
      if (dns_api_.restore_dns(last_device_.adapter_name) != 0) {
        result.success = false;
        result.error_message = "Darwin DNS restore failed";
        return result;
      }
      result.dns_removed = true;
      forget_dns(last_device_.adapter_name);
    }

    if (policy == CleanupPolicy::Full && utun_) {
      (void)adapter_name;
      utun_->stop();
      utun_.reset();
      last_device_ = {};
      forget_adapter(adapter_name);
      result.adapter_removed = true;
    }

    return result;
  }

  CleanupResult cleanup_resources(
      const std::vector<ManagedNetworkResource> &resources,
      CleanupPolicy policy) override {
    if (resources.empty()) {
      return cleanup({}, policy);
    }

    CleanupResult result;
    result.success = true;

    const bool remove_routes =
        policy == CleanupPolicy::Full || policy == CleanupPolicy::KeepAdapter ||
        policy == CleanupPolicy::RoutesOnly;
    if (remove_routes) {
      std::vector<exv::platform::NativeDarwinRoute> routes;
      for (const auto &resource : resources) {
        if (resource.type != "darwin_route") {
          continue;
        }
        exv::platform::NativeDarwinRoute route;
        if (parse_route_fact(resource.detail, &route)) {
          routes.push_back(route);
        }
      }
      for (auto it = routes.rbegin(); it != routes.rend(); ++it) {
        if (!route_api_.delete_route ||
            route_api_.delete_route(*it) != 0) {
          result.success = false;
          result.error_message = "deleting imported Darwin route failed";
          return result;
        }
        ++result.routes_removed;
      }
      forget_routes();
    }

    const bool remove_dns =
        policy == CleanupPolicy::Full || policy == CleanupPolicy::KeepAdapter ||
        policy == CleanupPolicy::DnsOnly;
    if (remove_dns) {
      for (const auto &resource : resources) {
        if (resource.type != "darwin_dns") {
          continue;
        }
        const std::string interface_name = interface_from_fact(resource.detail);
        if (!interface_name.empty() && dns_api_.restore_dns &&
            dns_api_.restore_dns(interface_name) == 0) {
          result.dns_removed = true;
          forget_dns(interface_name);
        } else {
          result.success = false;
          result.error_message = "restoring imported Darwin DNS failed";
          return result;
        }
      }
    }

    if (policy == CleanupPolicy::Full) {
      std::string adapter_name;
      for (const auto &resource : resources) {
        if (resource.type == "adapter") {
          adapter_name = resource.detail;
          break;
        }
      }
      if (!adapter_name.empty()) {
        if (utun_ && last_device_.adapter_name == adapter_name) {
          utun_->stop();
          utun_.reset();
          last_device_ = {};
          result.adapter_removed = true;
        } else if (dns_api_.disable_interface &&
                   dns_api_.disable_interface(adapter_name) == 0) {
          result.adapter_removed = true;
        } else {
          result.success = false;
          result.error_message = "disabling imported Darwin adapter failed";
          return result;
        }
        forget_adapter(adapter_name);
      }
    }

    return result;
  }

  std::vector<ManagedNetworkResource> managed_resources() const override {
    return managed_resources_;
  }

  bool device_exists(const std::string &adapter_name) const override {
    return utun_ && utun_->running() && last_device_.is_open &&
           (adapter_name.empty() || adapter_name == last_device_.adapter_name);
  }

private:
  void remember_adapter(const std::string &adapter_name) {
    if (adapter_name.empty()) {
      return;
    }
    for (const auto &resource : managed_resources_) {
      if (resource.type == "adapter" && resource.detail == adapter_name) {
        return;
      }
    }
    managed_resources_.push_back({"adapter", adapter_name});
  }

  void remember_routes(
      const std::vector<exv::platform::NativeDarwinRoute> &routes) {
    forget_routes();
    for (const auto &route : routes) {
      managed_resources_.push_back({"darwin_route", route_fact_detail(route)});
    }
  }

  void remember_dns(const std::string &interface_name,
                    const NativeDarwinDnsSettings &settings) {
    forget_dns(interface_name);
    managed_resources_.push_back({"darwin_dns",
                                  dns_fact_detail(interface_name, settings)});
  }

  bool has_dns_fact(const std::string &interface_name) const {
    for (const auto &resource : managed_resources_) {
      if (resource.type == "darwin_dns" &&
          interface_from_fact(resource.detail) == interface_name) {
        return true;
      }
    }
    return false;
  }

  void forget_routes() {
    std::vector<ManagedNetworkResource> kept;
    for (const auto &resource : managed_resources_) {
      if (resource.type != "darwin_route") {
        kept.push_back(resource);
      }
    }
    managed_resources_ = std::move(kept);
  }

  void forget_dns(const std::string &interface_name) {
    std::vector<ManagedNetworkResource> kept;
    for (const auto &resource : managed_resources_) {
      if (resource.type == "darwin_dns" &&
          (interface_name.empty() ||
           interface_from_fact(resource.detail) == interface_name)) {
        continue;
      }
      kept.push_back(resource);
    }
    managed_resources_ = std::move(kept);
  }

  void forget_adapter(const std::string &adapter_name) {
    std::vector<ManagedNetworkResource> kept;
    for (const auto &resource : managed_resources_) {
      if (resource.type == "adapter" &&
          (adapter_name.empty() || resource.detail == adapter_name)) {
        continue;
      }
      kept.push_back(resource);
    }
    managed_resources_ = std::move(kept);
  }

  exv::platform::NativeUtunApi utun_api_;
  exv::platform::NativeDarwinRouteApi route_api_;
  NativeDarwinDnsApi dns_api_;
  std::unique_ptr<exv::platform::NativeUtun> utun_;
  std::unique_ptr<exv::platform::NativeDarwinRouteConfig> route_config_;
  TunnelDeviceDescriptor last_device_;
  std::vector<ManagedNetworkResource> managed_resources_;
};

} // namespace

NativeDarwinDnsApi default_native_darwin_dns_api() {
  NativeDarwinDnsApi api;
  api.apply_dns = [](const std::string &interface_name,
                     const NativeDarwinDnsSettings &settings) {
    if (interface_name.empty() || settings.servers.empty()) {
      return EINVAL;
    }
    const std::string script = scutil_dns_script(interface_name, settings);
    const std::string command =
        "printf %s " + exv::platform::shell_quote(script) +
        " | /usr/sbin/scutil";
    return exv::platform::run_command(command);
  };
  api.restore_dns = [](const std::string &interface_name) {
    if (interface_name.empty()) {
      return EINVAL;
    }
    const std::string script =
        "remove State:/Network/Service/exv-" + interface_name + "/DNS\n";
    const std::string command =
        "printf %s " + exv::platform::shell_quote(script) +
        " | /usr/sbin/scutil";
    return exv::platform::run_command(command);
  };
  api.disable_interface = [](const std::string &interface_name) {
    if (interface_name.empty()) {
      return EINVAL;
    }
    return exv::platform::run_command(
        "/sbin/ifconfig " + exv::platform::shell_quote(interface_name) +
        " down >/dev/null 2>&1");
  };
  return api;
}

std::unique_ptr<PlatformNetworkOps> create_darwin_platform_network_ops() {
  return create_darwin_platform_network_ops(
      exv::platform::default_native_utun_api(),
      exv::platform::default_native_darwin_route_api(),
      default_native_darwin_dns_api());
}

std::unique_ptr<PlatformNetworkOps> create_darwin_platform_network_ops(
    exv::platform::NativeUtunApi utun_api,
    exv::platform::NativeDarwinRouteApi route_api) {
  return create_darwin_platform_network_ops(
      std::move(utun_api), std::move(route_api),
      default_native_darwin_dns_api());
}

std::unique_ptr<PlatformNetworkOps> create_darwin_platform_network_ops(
    exv::platform::NativeUtunApi utun_api,
    exv::platform::NativeDarwinRouteApi route_api,
    NativeDarwinDnsApi dns_api) {
  return std::make_unique<DarwinPlatformNetworkOps>(
      std::move(utun_api), std::move(route_api), std::move(dns_api));
}

} // namespace exv::platform
