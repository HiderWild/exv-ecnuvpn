#include "platform/darwin/native_route_config.hpp"

#include <algorithm>
#include <cerrno>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <limits>
#include <set>
#include <sstream>
#include <utility>
#include <vector>

#if defined(__APPLE__)
#include <arpa/inet.h>
#include <net/if.h>
#include <net/if_dl.h>
#include <net/route.h>
#include <netinet/in.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <unistd.h>
#endif

namespace ecnuvpn {
namespace platform {
namespace {

constexpr int kNoError = 0;
constexpr int kMinimumUsableMtu = 1200;
constexpr int kMaximumMtu = 1500;
// Begin inlined from platform/darwin/native_route_config_model include-unit
NativeDarwinRouteConfigResult
failure(NativeDarwinRouteConfigError error, std::string message = {},
        std::string target = {}, int system_error = 0) {
  NativeDarwinRouteConfigResult result;
  result.error = error;
  result.message = std::move(message);
  result.target = std::move(target);
  result.system_error = system_error;
  return result;
}

bool has_required_api(const NativeDarwinRouteApi &api) {
  return static_cast<bool>(api.set_interface_mtu) &&
         static_cast<bool>(api.get_best_route) &&
         static_cast<bool>(api.add_route) &&
         static_cast<bool>(api.delete_route);
}

std::string trim_ascii(const std::string &value) {
  std::size_t first = 0;
  while (first < value.size() &&
         (value[first] == ' ' || value[first] == '\t' ||
          value[first] == '\r' || value[first] == '\n'))
    ++first;

  std::size_t last = value.size();
  while (last > first &&
         (value[last - 1] == ' ' || value[last - 1] == '\t' ||
          value[last - 1] == '\r' || value[last - 1] == '\n'))
    --last;

  return value.substr(first, last - first);
}

bool parse_ipv4_host_order(const std::string &value, std::uint32_t *out) {
  if (!out)
    return false;

  std::uint32_t octets[4] = {};
  std::size_t offset = 0;
  for (int i = 0; i < 4; ++i) {
    if (offset >= value.size() || value[offset] < '0' ||
        value[offset] > '9')
      return false;

    int octet = 0;
    int digits = 0;
    while (offset < value.size() && value[offset] >= '0' &&
           value[offset] <= '9') {
      octet = octet * 10 + (value[offset] - '0');
      if (octet > 255)
        return false;
      ++digits;
      ++offset;
    }

    if (digits == 0)
      return false;

    octets[i] = static_cast<std::uint32_t>(octet);
    if (i < 3) {
      if (offset >= value.size() || value[offset] != '.')
        return false;
      ++offset;
    }
  }

  if (offset != value.size())
    return false;

  *out = (octets[0] << 24) | (octets[1] << 16) | (octets[2] << 8) |
         octets[3];
  return true;
}

std::string ipv4_from_host_order(std::uint32_t value) {
  std::ostringstream out;
  out << ((value >> 24) & 0xff) << '.' << ((value >> 16) & 0xff) << '.'
      << ((value >> 8) & 0xff) << '.' << (value & 0xff);
  return out.str();
}

bool parse_prefix(const std::string &value, int *out) {
  if (value.empty() || !out)
    return false;

  int prefix = 0;
  for (char ch : value) {
    if (ch < '0' || ch > '9')
      return false;
    prefix = prefix * 10 + (ch - '0');
    if (prefix > 32)
      return false;
  }

  *out = prefix;
  return true;
}

std::uint32_t mask_from_prefix(int prefix) {
  if (prefix <= 0)
    return 0;
  if (prefix >= 32)
    return std::numeric_limits<std::uint32_t>::max();
  return std::numeric_limits<std::uint32_t>::max() << (32 - prefix);
}

bool normalize_cidr(const std::string &input, int default_prefix,
                    NativeDarwinRoute *route) {
  if (!route)
    return false;

  const std::string value = trim_ascii(input);
  if (value.empty())
    return false;

  const std::size_t slash = value.find('/');
  if (slash != std::string::npos &&
      value.find('/', slash + 1) != std::string::npos)
    return false;

  const std::string address =
      slash == std::string::npos ? value : value.substr(0, slash);
  int prefix = default_prefix;
  if (slash != std::string::npos &&
      !parse_prefix(value.substr(slash + 1), &prefix))
    return false;

  std::uint32_t host_order = 0;
  if (!parse_ipv4_host_order(address, &host_order))
    return false;

  const std::uint32_t mask = mask_from_prefix(prefix);
  const std::uint32_t network = host_order & mask;
  route->destination = ipv4_from_host_order(network);
  route->netmask = ipv4_from_host_order(mask);
  route->prefix_length = prefix;
  route->cidr = route->destination + "/" + std::to_string(prefix);
  return true;
}

int choose_effective_mtu(int tunnel_mtu, int configured_mtu) {
  if (tunnel_mtu >= kMinimumUsableMtu && tunnel_mtu <= kMaximumMtu)
    return tunnel_mtu;
  if (configured_mtu >= kMinimumUsableMtu && configured_mtu <= kMaximumMtu)
    return configured_mtu;
  return 0;
}

std::string effective_interface_name(
    const NativeDarwinRouteConfigOptions &options,
    const vpn_engine::TunnelMetadata &metadata) {
  if (!options.interface_name.empty())
    return options.interface_name;
  return metadata.interface_name;
}

std::uint32_t effective_interface_index(
    const NativeDarwinRouteConfigOptions &options,
    const vpn_engine::TunnelMetadata &metadata) {
  if (options.interface_index != 0)
    return options.interface_index;
  if (metadata.interface_index > 0)
    return static_cast<std::uint32_t>(metadata.interface_index);
  return 0;
}

std::uint32_t stable_interface_index(const NativeDarwinRouteApi &api,
                                     const std::string &interface_name,
                                     std::uint32_t known_index) {
  if (known_index != 0)
    return known_index;
  if (interface_name.empty() || !api.interface_index_from_name)
    return 0;
  return api.interface_index_from_name(interface_name);
}

void scope_route_message_to_interface(NativeDarwinRoute *route) {
  if (!route)
    return;

  route->message_interface_scoped = true;
  route->message_interface_index = route->interface_index;
}

bool plan_routes(const vpn_engine::TunnelMetadata &metadata,
                 std::vector<NativeDarwinRoute> *routes,
                 std::string *invalid_target) {
  if (!routes)
    return false;
  routes->clear();

  std::set<std::string> seen;

  auto append_route = [&](const std::string &cidr, bool server_bypass,
                          int default_prefix) {
    NativeDarwinRoute route;
    if (!normalize_cidr(cidr, default_prefix, &route)) {
      if (invalid_target)
        *invalid_target = cidr;
      return false;
    }
    route.server_bypass = server_bypass;
    if (!seen.insert(route.cidr).second)
      return true;
    routes->push_back(std::move(route));
    return true;
  };

  for (const std::string &bypass : metadata.server_bypass_ips) {
    if (!append_route(bypass, true, 32))
      return false;
  }
  for (const std::string &split : metadata.routes) {
    if (!append_route(split, false, 32))
      return false;
  }

  return true;
}

int unsupported_native_api_error() { return ENOSYS; }
// End inlined from platform/darwin/native_route_config_model include-unit
#if defined(__APPLE__)
// Begin inlined from platform/darwin/native_route_config_socket include-unit
int errno_or_io_error() { return errno == 0 ? EIO : errno; }

std::size_t aligned_sockaddr_length(const sockaddr *address) {
  if (!address)
    return 0;

  std::size_t length = address->sa_len;
  if (length == 0)
    length = sizeof(sockaddr);

  const std::size_t alignment = sizeof(long);
  return (length + alignment - 1) & ~(alignment - 1);
}

void append_sockaddr(std::vector<char> *message, const sockaddr *address) {
  if (!message || !address)
    return;

  const std::size_t length =
      address->sa_len == 0 ? sizeof(sockaddr) : address->sa_len;
  const std::size_t aligned_length = aligned_sockaddr_length(address);
  const std::size_t offset = message->size();
  message->resize(offset + aligned_length);
  std::memset(message->data() + offset, 0, aligned_length);
  std::memcpy(message->data() + offset, address, length);
}

sockaddr_in make_inet_sockaddr(std::uint32_t host_order) {
  sockaddr_in address{};
  address.sin_len = sizeof(sockaddr_in);
  address.sin_family = AF_INET;
  address.sin_addr.s_addr = htonl(host_order);
  return address;
}

sockaddr_dl make_link_sockaddr(std::uint32_t interface_index) {
  sockaddr_dl address{};
  address.sdl_len = sizeof(sockaddr_dl);
  address.sdl_family = AF_LINK;
  address.sdl_index = static_cast<unsigned short>(interface_index);
  return address;
}

std::string sockaddr_in_to_ipv4(const sockaddr_in *address) {
  if (!address)
    return {};
  return ipv4_from_host_order(ntohl(address->sin_addr.s_addr));
}

void copy_if_name(const sockaddr_dl *address, NativeDarwinUpstreamRoute *route) {
  if (!address || !route)
    return;

  if (address->sdl_index != 0)
    route->interface_index = static_cast<std::uint32_t>(address->sdl_index);
  if (address->sdl_nlen > 0)
    route->interface_name.assign(address->sdl_data, address->sdl_nlen);
}

void fill_interface_name_from_index(NativeDarwinUpstreamRoute *route) {
  if (!route || !route->interface_name.empty() || route->interface_index == 0)
    return;

  char name[IF_NAMESIZE] = {};
  if (if_indextoname(route->interface_index, name))
    route->interface_name = name;
}

int write_route_message(int route_socket, int type, int flags, int addrs,
                        const std::vector<const sockaddr *> &sockaddrs,
                        NativeDarwinUpstreamRoute *queried_route = nullptr,
                        std::uint32_t message_interface_index = 0) {
  static std::uint32_t sequence = 0;

  std::vector<char> message(sizeof(rt_msghdr));
  for (const sockaddr *address : sockaddrs)
    append_sockaddr(&message, address);

  rt_msghdr *header = reinterpret_cast<rt_msghdr *>(message.data());
  header->rtm_msglen = static_cast<unsigned short>(message.size());
  header->rtm_version = RTM_VERSION;
  header->rtm_type = type;
  header->rtm_flags = flags;
  header->rtm_addrs = addrs;
  header->rtm_seq = ++sequence;
  header->rtm_pid = getpid();
  if (message_interface_index != 0)
    header->rtm_index = static_cast<unsigned short>(message_interface_index);

  if (write(route_socket, message.data(), message.size()) < 0)
    return errno_or_io_error();

  if (!queried_route)
    return kNoError;

  char buffer[2048] = {};
  for (;;) {
    const ssize_t read_count = read(route_socket, buffer, sizeof(buffer));
    if (read_count < 0)
      return errno_or_io_error();
    if (read_count < static_cast<ssize_t>(sizeof(rt_msghdr)))
      continue;

    const rt_msghdr *reply = reinterpret_cast<const rt_msghdr *>(buffer);
    if (reply->rtm_seq != header->rtm_seq || reply->rtm_pid != header->rtm_pid)
      continue;
    if (reply->rtm_errno != 0)
      return reply->rtm_errno;

    const sockaddr *route_addrs[RTAX_MAX] = {};
    const char *cursor = buffer + sizeof(rt_msghdr);
    const char *end = buffer + read_count;
    for (int index = 0; index < RTAX_MAX; ++index) {
      if ((reply->rtm_addrs & (1 << index)) == 0)
        continue;
      if (cursor >= end)
        break;
      const sockaddr *address = reinterpret_cast<const sockaddr *>(cursor);
      route_addrs[index] = address;
      cursor += aligned_sockaddr_length(address);
    }

    if (route_addrs[RTAX_GATEWAY] &&
        route_addrs[RTAX_GATEWAY]->sa_family == AF_INET) {
      queried_route->gateway = sockaddr_in_to_ipv4(
          reinterpret_cast<const sockaddr_in *>(route_addrs[RTAX_GATEWAY]));
    } else if (route_addrs[RTAX_GATEWAY] &&
               route_addrs[RTAX_GATEWAY]->sa_family == AF_LINK) {
      copy_if_name(reinterpret_cast<const sockaddr_dl *>(
                       route_addrs[RTAX_GATEWAY]),
                   queried_route);
    }

    if (route_addrs[RTAX_IFP] &&
        route_addrs[RTAX_IFP]->sa_family == AF_LINK) {
      copy_if_name(reinterpret_cast<const sockaddr_dl *>(route_addrs[RTAX_IFP]),
                   queried_route);
    }
    fill_interface_name_from_index(queried_route);
    return kNoError;
  }
}

int open_route_socket() {
  const int route_socket = socket(PF_ROUTE, SOCK_RAW, AF_UNSPEC);
  return route_socket;
}

int set_interface_mtu_native(const std::string &interface_name, int mtu) {
  const int sock = socket(AF_INET, SOCK_DGRAM, 0);
  if (sock < 0)
    return errno_or_io_error();

  ifreq request{};
  std::snprintf(request.ifr_name, sizeof(request.ifr_name), "%s",
                interface_name.c_str());
  request.ifr_mtu = mtu;
  const int result = ioctl(sock, SIOCSIFMTU, &request);
  const int saved_errno = result == 0 ? kNoError : errno_or_io_error();
  close(sock);
  return saved_errno;
}

int get_best_route_native(const std::string &destination,
                          NativeDarwinUpstreamRoute *route) {
  if (!route)
    return EINVAL;

  std::uint32_t destination_host_order = 0;
  if (!parse_ipv4_host_order(destination, &destination_host_order))
    return EINVAL;

  const int route_socket = open_route_socket();
  if (route_socket < 0)
    return errno_or_io_error();

  sockaddr_in destination_address = make_inet_sockaddr(destination_host_order);
  const int result = write_route_message(
      route_socket, RTM_GET, RTF_UP | RTF_HOST, RTA_DST,
      {reinterpret_cast<const sockaddr *>(&destination_address)}, route);
  close(route_socket);
  return result;
}

std::uint32_t route_interface_index(const NativeDarwinRoute &route) {
  if (route.interface_index != 0)
    return route.interface_index;
  if (route.interface_name.empty())
    return 0;
  return if_nametoindex(route.interface_name.c_str());
}

std::uint32_t route_message_interface_index(const NativeDarwinRoute &route) {
  if (!route.message_interface_scoped)
    return 0;
  if (route.message_interface_index != 0)
    return route.message_interface_index;
  return route_interface_index(route);
}

int update_route_native(int message_type, const NativeDarwinRoute &route) {
  std::uint32_t destination_host_order = 0;
  std::uint32_t netmask_host_order = 0;
  if (!parse_ipv4_host_order(route.destination, &destination_host_order) ||
      !parse_ipv4_host_order(route.netmask, &netmask_host_order))
    return EINVAL;

  sockaddr_in destination = make_inet_sockaddr(destination_host_order);
  sockaddr_in netmask = make_inet_sockaddr(netmask_host_order);
  sockaddr_in gateway_inet{};
  sockaddr_dl gateway_link{};
  const sockaddr *gateway = nullptr;

  int flags = RTF_UP | RTF_STATIC;
  if (route.prefix_length == 32)
    flags |= RTF_HOST;

  const std::uint32_t message_interface_index =
      route_message_interface_index(route);
  if (route.message_interface_scoped) {
    if (message_interface_index == 0)
      return ENXIO;
#if defined(RTF_IFSCOPE)
    flags |= RTF_IFSCOPE;
#else
    return ENOTSUP;
#endif
  }

  if (!route.gateway.empty()) {
    std::uint32_t gateway_host_order = 0;
    if (!parse_ipv4_host_order(route.gateway, &gateway_host_order))
      return EINVAL;
    gateway_inet = make_inet_sockaddr(gateway_host_order);
    gateway = reinterpret_cast<const sockaddr *>(&gateway_inet);
    flags |= RTF_GATEWAY;
  } else {
    const std::uint32_t interface_index = route_interface_index(route);
    if (interface_index == 0)
      return ENXIO;
    gateway_link = make_link_sockaddr(interface_index);
    gateway = reinterpret_cast<const sockaddr *>(&gateway_link);
  }

  const int route_socket = open_route_socket();
  if (route_socket < 0)
    return errno_or_io_error();

  const int result = write_route_message(
      route_socket, message_type, flags, RTA_DST | RTA_GATEWAY | RTA_NETMASK,
      {reinterpret_cast<const sockaddr *>(&destination), gateway,
       reinterpret_cast<const sockaddr *>(&netmask)},
      nullptr, message_interface_index);
  close(route_socket);
  return result;
}
// End inlined from platform/darwin/native_route_config_socket include-unit
#endif

} // namespace
// Begin inlined from platform/darwin/native_route_config_public include-unit
const char *
native_darwin_route_config_error_code(NativeDarwinRouteConfigError error) {
  switch (error) {
  case NativeDarwinRouteConfigError::none:
    return "none";
  case NativeDarwinRouteConfigError::api_missing:
    return "api_missing";
  case NativeDarwinRouteConfigError::invalid_interface:
    return "invalid_interface";
  case NativeDarwinRouteConfigError::invalid_mtu:
    return "invalid_mtu";
  case NativeDarwinRouteConfigError::invalid_route:
    return "invalid_route";
  case NativeDarwinRouteConfigError::mtu_set_failed:
    return "mtu_set_failed";
  case NativeDarwinRouteConfigError::upstream_route_failed:
    return "upstream_route_failed";
  case NativeDarwinRouteConfigError::route_add_failed:
    return "route_add_failed";
  case NativeDarwinRouteConfigError::route_delete_failed:
    return "route_delete_failed";
  }
  return "unknown";
}

NativeDarwinRouteApi default_native_darwin_route_api() {
  NativeDarwinRouteApi api;
#if defined(__APPLE__)
  api.set_interface_mtu =
      [](const std::string &interface_name, int mtu) {
        return set_interface_mtu_native(interface_name, mtu);
      };
  api.get_best_route =
      [](const std::string &destination, NativeDarwinUpstreamRoute &route) {
        return get_best_route_native(destination, &route);
      };
  api.add_route = [](const NativeDarwinRoute &route) {
    return update_route_native(RTM_ADD, route);
  };
  api.delete_route = [](const NativeDarwinRoute &route) {
    return update_route_native(RTM_DELETE, route);
  };
  api.interface_index_from_name = [](const std::string &interface_name) {
    return if_nametoindex(interface_name.c_str());
  };
#else
  api.set_interface_mtu = [](const std::string &, int) {
    return unsupported_native_api_error();
  };
  api.get_best_route = [](const std::string &, NativeDarwinUpstreamRoute &) {
    return unsupported_native_api_error();
  };
  api.add_route = [](const NativeDarwinRoute &) {
    return unsupported_native_api_error();
  };
  api.delete_route = [](const NativeDarwinRoute &) {
    return unsupported_native_api_error();
  };
  api.interface_index_from_name = [](const std::string &) {
    return static_cast<std::uint32_t>(0);
  };
#endif
  return api;
}

NativeDarwinRouteConfig::NativeDarwinRouteConfig(
    NativeDarwinRouteApi api, NativeDarwinRouteConfigOptions options)
    : api_(std::move(api)), options_(std::move(options)) {}

NativeDarwinRouteConfigResult NativeDarwinRouteConfig::configure(
    const vpn_engine::TunnelMetadata &metadata) {
  if (!has_required_api(api_))
    return failure(NativeDarwinRouteConfigError::api_missing,
                   "native Darwin route API table is incomplete");

  const std::string interface_name =
      effective_interface_name(options_, metadata);
  std::uint32_t interface_index =
      effective_interface_index(options_, metadata);
  if (interface_name.empty())
    return failure(NativeDarwinRouteConfigError::invalid_interface,
                   "missing utun interface name");

  const int mtu = choose_effective_mtu(metadata.mtu, options_.configured_mtu);
  if (mtu == 0)
    return failure(NativeDarwinRouteConfigError::invalid_mtu,
                   "no usable tunnel or configured MTU");

  NativeDarwinRouteApi::ErrorCode error =
      api_.set_interface_mtu(interface_name, mtu);
  if (error != kNoError)
    return failure(NativeDarwinRouteConfigError::mtu_set_failed,
                   "setting utun interface MTU failed", interface_name, error);
  effective_mtu_ = mtu;

  std::vector<NativeDarwinRoute> routes;
  std::string invalid_target;
  if (!plan_routes(metadata, &routes, &invalid_target))
    return failure(NativeDarwinRouteConfigError::invalid_route,
                   "invalid IPv4 route in tunnel metadata", invalid_target);

  for (NativeDarwinRoute &route : routes) {
    if (route.server_bypass) {
      NativeDarwinUpstreamRoute upstream_route;
      error = api_.get_best_route(route.destination, upstream_route);
      if (error != kNoError)
        return failure(NativeDarwinRouteConfigError::upstream_route_failed,
                       "querying upstream route failed", route.cidr, error);
      route.interface_name = upstream_route.interface_name;
      route.interface_index = upstream_route.interface_index;
      route.gateway = upstream_route.gateway;
      scope_route_message_to_interface(&route);
    } else {
      interface_index =
          stable_interface_index(api_, interface_name, interface_index);
      route.interface_name = interface_name;
      route.interface_index = interface_index;
      route.gateway.clear();
      scope_route_message_to_interface(&route);
    }

    error = api_.add_route(route);
    if (error != kNoError)
      return failure(NativeDarwinRouteConfigError::route_add_failed,
                     "adding Darwin route failed", route.cidr, error);

    owned_routes_.push_back(route);
  }

  NativeDarwinRouteConfigResult result;
  result.effective_mtu = effective_mtu_;
  return result;
}

NativeDarwinRouteConfigResult NativeDarwinRouteConfig::cleanup() {
  if (owned_routes_.empty())
    return {};

  if (!api_.delete_route)
    return failure(NativeDarwinRouteConfigError::api_missing,
                   "native Darwin route delete API is missing");

  while (!owned_routes_.empty()) {
    NativeDarwinRoute route = owned_routes_.back();
    NativeDarwinRouteApi::ErrorCode error = api_.delete_route(route);
    if (error != kNoError)
      return failure(NativeDarwinRouteConfigError::route_delete_failed,
                     "deleting Darwin route failed", route.cidr, error);
    owned_routes_.pop_back();
  }

  return {};
}

const std::vector<NativeDarwinRoute> &
NativeDarwinRouteConfig::owned_routes() const {
  return owned_routes_;
}

int NativeDarwinRouteConfig::effective_mtu() const { return effective_mtu_; }
// End inlined from platform/darwin/native_route_config_public include-unit
} // namespace platform
} // namespace ecnuvpn
