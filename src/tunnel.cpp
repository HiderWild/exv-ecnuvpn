#include "tunnel.hpp"

#include "logger.hpp"
#include "platform/common/tunnel_script.hpp"
#include "utils.hpp"

#ifndef _WIN32
#include <arpa/inet.h>
#include <netdb.h>
#include <sys/stat.h>
#else
#include <winsock2.h>
#include <ws2tcpip.h>
#endif

#include <cstdlib>
#include <set>
#include <sstream>
#include <vector>

namespace ecnuvpn {
namespace tunnel {
namespace {

std::string extract_host(const std::string &server) {
  std::string value = server;
  std::size_t scheme_pos = value.find("://");
  std::size_t host_start =
      scheme_pos == std::string::npos ? 0 : scheme_pos + 3;
  std::size_t host_end = value.find_first_of("/?#", host_start);
  std::string authority = value.substr(host_start, host_end - host_start);
  if (authority.empty())
    return "";

  if (authority.front() == '[') {
    std::size_t close = authority.find(']');
    if (close != std::string::npos)
      return authority.substr(1, close - 1);
    return authority;
  }

  std::size_t colon = authority.rfind(':');
  if (colon != std::string::npos && authority.find(':') == colon)
    return authority.substr(0, colon);
  return authority;
}

bool parse_ipv4(const std::string &value, uint32_t *out) {
  in_addr addr{};
  if (inet_pton(AF_INET, value.c_str(), &addr) != 1)
    return false;
  if (out)
    *out = ntohl(addr.s_addr);
  return true;
}

std::vector<std::string> resolve_server_ips(const Config &cfg) {
  std::vector<std::string> result;
  std::string host = extract_host(cfg.server);
  if (host.empty())
    return result;

  std::set<std::string> unique;
  uint32_t literal = 0;
  if (parse_ipv4(host, &literal))
    unique.insert(host);

  addrinfo hints{};
  hints.ai_family = AF_INET;
  hints.ai_socktype = SOCK_STREAM;

  addrinfo *addresses = nullptr;
  if (getaddrinfo(host.c_str(), nullptr, &hints, &addresses) == 0) {
    for (addrinfo *cursor = addresses; cursor != nullptr;
         cursor = cursor->ai_next) {
      auto *sockaddr_v4 = reinterpret_cast<sockaddr_in *>(cursor->ai_addr);
      char buffer[INET_ADDRSTRLEN] = {0};
      if (inet_ntop(AF_INET, &sockaddr_v4->sin_addr, buffer, sizeof(buffer)))
        unique.insert(buffer);
    }
    freeaddrinfo(addresses);
  }

  result.assign(unique.begin(), unique.end());
  return result;
}

bool route_contains_ip(const std::string &route, const std::string &ip) {
  uint32_t ip_value = 0;
  if (!parse_ipv4(ip, &ip_value))
    return false;

  std::size_t slash = route.find('/');
  if (slash == std::string::npos) {
    uint32_t route_ip = 0;
    return parse_ipv4(route, &route_ip) && route_ip == ip_value;
  }

  uint32_t network = 0;
  if (!parse_ipv4(route.substr(0, slash), &network))
    return false;

  int prefix = -1;
  try {
    prefix = std::stoi(route.substr(slash + 1));
  } catch (...) {
    return false;
  }

  if (prefix < 0 || prefix > 32)
    return false;

  uint32_t mask = prefix == 0 ? 0 : (~uint32_t(0) << (32 - prefix));
  return (network & mask) == (ip_value & mask);
}

std::vector<std::string> find_server_route_exceptions(const Config &cfg) {
  std::vector<std::string> server_ips = resolve_server_ips(cfg);
  std::set<std::string> result;

  for (const auto &server_ip : server_ips) {
    for (const auto &route : cfg.routes) {
      if (route_contains_ip(route, server_ip)) {
        result.insert(server_ip);
        break;
      }
    }
  }

  return std::vector<std::string>(result.begin(), result.end());
}

platform::TunnelScriptContext make_tunnel_script_context(const Config &cfg) {
  platform::TunnelScriptContext context;
  context.route_ready_path = utils::get_route_ready_path();
  context.custom_routes = cfg.routes;
  context.server_route_exceptions = find_server_route_exceptions(cfg);
  context.configured_mtu = cfg.mtu;
  context.has_runtime_owner = utils::has_runtime_owner();
  context.runtime_owner_uid =
      static_cast<unsigned int>(utils::get_runtime_owner_uid());
  context.runtime_owner_gid =
      static_cast<unsigned int>(utils::get_runtime_owner_gid());
  return context;
}

} // namespace

std::string generate(const Config &cfg) {
  return platform::generate_tunnel_script(make_tunnel_script_context(cfg));
}

bool write_script(const Config &cfg) {
  std::string path = utils::get_tunnel_path();
  platform::TunnelScriptContext context = make_tunnel_script_context(cfg);
  std::string content = platform::generate_tunnel_script(context);

  for (const auto &server_ip : context.server_route_exceptions) {
    logger::warn("Preserving upstream route for VPN server IP: " + server_ip);
  }

  if (!utils::write_file(path, content)) {
    utils::print_error("Failed to write tunnel script: " + path);
    logger::error("Failed to write tunnel script: " + path);
    return false;
  }

#ifndef _WIN32
  if (chmod(path.c_str(), 0755) != 0) {
    utils::print_error("Failed to set executable permission on: " + path);
    logger::error("Failed to chmod tunnel script: " + path);
    return false;
  }
#endif

  utils::sync_owner(path);

  logger::info("Tunnel script generated: " + path);
  return true;
}

int run_script_hook() {
#ifdef _WIN32
  const char *home = std::getenv("ECNUVPN_HOME");
  const char *config_dir = std::getenv("ECNUVPN_CONFIG_DIR");
  if ((home && *home) || (config_dir && *config_dir)) {
    utils::set_runtime_path_override(home ? home : "",
                                     config_dir ? config_dir : "");
  }
#endif
  logger::init();
  Config cfg = config::load();
  platform::TunnelScriptContext context = make_tunnel_script_context(cfg);
  return platform::run_tunnel_script(context);
}

bool configure_from_runtime_log(const Config &cfg) {
  platform::TunnelScriptContext context = make_tunnel_script_context(cfg);
  return platform::configure_from_openconnect_log(context,
                                                 utils::expand_home(cfg.log_file));
}

void cleanup_routes() {
#ifndef _WIN32
  Config cfg = config::load();
  if (cfg.routes.empty()) {
    logger::info("No routes configured, skipping route cleanup");
    return;
  }

  platform::TunnelScriptContext context = make_tunnel_script_context(cfg);
  logger::info("Cleaning up VPN routes (" + std::to_string(cfg.routes.size()) +
               " configured)");

  platform::cleanup_tunnel_routes(context);

  logger::info("Route cleanup complete (" +
               std::to_string(cfg.routes.size()) + " routes removed)");
#endif
}

} // namespace tunnel
} // namespace ecnuvpn
