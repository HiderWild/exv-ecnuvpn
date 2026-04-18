#include "tunnel.hpp"
#include "logger.hpp"
#include "utils.hpp"

#include <arpa/inet.h>
#include <fstream>
#include <netdb.h>
#include <set>
#include <sstream>
#include <sys/stat.h>
#include <vector>

namespace ecnuvpn {
namespace tunnel {

static std::string extract_host(const std::string &server) {
  std::string value = server;
  std::size_t scheme_pos = value.find("://");
  std::size_t host_start = scheme_pos == std::string::npos ? 0 : scheme_pos + 3;
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

static bool parse_ipv4(const std::string &value, uint32_t *out) {
  in_addr addr {};
  if (inet_pton(AF_INET, value.c_str(), &addr) != 1)
    return false;
  if (out)
    *out = ntohl(addr.s_addr);
  return true;
}

static std::vector<std::string> resolve_server_ips(const Config &cfg) {
  std::vector<std::string> result;
  std::string host = extract_host(cfg.server);
  if (host.empty())
    return result;

  std::set<std::string> unique;
  uint32_t literal = 0;
  if (parse_ipv4(host, &literal))
    unique.insert(host);

  addrinfo hints {};
  hints.ai_family = AF_INET;
  hints.ai_socktype = SOCK_STREAM;

  addrinfo *addresses = nullptr;
  if (getaddrinfo(host.c_str(), nullptr, &hints, &addresses) == 0) {
    for (addrinfo *cursor = addresses; cursor != nullptr; cursor = cursor->ai_next) {
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

static bool route_contains_ip(const std::string &route, const std::string &ip) {
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

static std::vector<std::string> find_server_route_exceptions(const Config &cfg) {
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

std::string generate(const Config &cfg) {
  std::ostringstream ss;
  std::vector<std::string> server_route_exceptions =
      find_server_route_exceptions(cfg);
  bool has_runtime_owner = utils::has_runtime_owner();
  uid_t runtime_owner_uid = utils::get_runtime_owner_uid();
  gid_t runtime_owner_gid = utils::get_runtime_owner_gid();

  ss << "#!/bin/bash\n";
  ss << "\n";
  ss << "# =================================================================\n";
  ss << "# ECNU-VPN Split Tunnel Script (Auto-generated)\n";
  ss << "# Do NOT edit manually — regenerated on each VPN start\n";
  ss << "# =================================================================\n";
  ss << "\n";
  ss << "READY_FILE=\"" << utils::get_route_ready_path() << "\"\n";
  if (has_runtime_owner) {
    ss << "OWNER_UID=\"" << runtime_owner_uid << "\"\n";
    ss << "OWNER_GID=\"" << runtime_owner_gid << "\"\n";
  }
  ss << "rm -f \"$READY_FILE\"\n";
  ss << "\n";
  ss << "# Only run on connect\n";
  ss << "if [ \"$reason\" != \"connect\" ]; then\n";
  ss << "    exit 0\n";
  ss << "fi\n";
  ss << "\n";
  ss << "echo \">>> [VPN] Connection established, configuring network...\"\n";
  ss << "echo \">>> [VPN] Interface: $TUNDEV | Internal IP: "
        "$INTERNAL_IP4_ADDRESS\"\n";
  ss << "\n";
  ss << "# Activate virtual interface\n";
  ss << "ifconfig \"$TUNDEV\" \"$INTERNAL_IP4_ADDRESS\" "
        "\"$INTERNAL_IP4_ADDRESS\" netmask 255.255.255.255 up >/dev/null 2>&1\n";
  ss << "if [ $? -ne 0 ]; then\n";
  ss << "    echo \">>> [VPN] Failed to activate interface: $TUNDEV\"\n";
  ss << "    exit 1\n";
  ss << "fi\n";
  ss << "\n";
  if (!server_route_exceptions.empty()) {
    ss << "DEFAULT_ROUTE=$(route -n get default 2>/dev/null)\n";
    ss << "DEFAULT_GATEWAY=$(printf '%s\\n' \"$DEFAULT_ROUTE\" | awk '/gateway:/{print $2; exit}')\n";
    ss << "DEFAULT_INTERFACE=$(printf '%s\\n' \"$DEFAULT_ROUTE\" | awk '/interface:/{print $2; exit}')\n";
    ss << "if [ -n \"$DEFAULT_GATEWAY\" ] && [ -n \"$DEFAULT_INTERFACE\" ]; then\n";
    for (const auto &server_ip : server_route_exceptions) {
      ss << "    route -n delete \"" << server_ip << "\" >/dev/null 2>&1\n";
      ss << "    route -n add -host \"" << server_ip
         << "\" \"$DEFAULT_GATEWAY\" >/dev/null 2>&1\n";
      ss << "    if [ $? -eq 0 ]; then\n";
      ss << "        echo \"  [+] Server route preserved: " << server_ip
         << " via $DEFAULT_INTERFACE\"\n";
      ss << "    else\n";
      ss << "        echo \"  [-] Server route warning: " << server_ip
         << " (failed to preserve upstream path)\"\n";
      ss << "    fi\n";
    }
    ss << "fi\n";
    ss << "\n";
  }
  ss << "# Split tunnel routes\n";
  ss << "echo \">>> [VPN] Adding split tunnel routes...\"\n";
  ss << "\n";

  for (const auto &route : cfg.routes) {
    ss << "route -n delete \"" << route << "\" >/dev/null 2>&1\n";
    ss << "route -n add \"" << route
       << "\" -interface \"$TUNDEV\" >/dev/null 2>&1\n";
    ss << "if [ $? -eq 0 ]; then\n";
    ss << "    echo \"  [+] Route added: " << route << "\"\n";
    ss << "else\n";
    ss << "    echo \"  [-] Route warning: " << route
       << " (failed to refresh)\"\n";
    ss << "fi\n";
    ss << "\n";
  }

  ss << "printf '%s\\n%s\\n' \"$TUNDEV\" \"$INTERNAL_IP4_ADDRESS\" > \"$READY_FILE\"\n";
  ss << "if [ $? -ne 0 ]; then\n";
  ss << "    echo \">>> [VPN] Failed to write route-ready marker.\"\n";
  ss << "    exit 1\n";
  ss << "fi\n";
  if (has_runtime_owner) {
    ss << "chown \"$OWNER_UID\":\"$OWNER_GID\" \"$READY_FILE\" >/dev/null 2>&1\n";
    ss << "chmod 0644 \"$READY_FILE\" >/dev/null 2>&1\n";
  }
  ss << "\n";
  ss << "echo \">>> [VPN] Network configuration complete!\"\n";
  ss << "echo \">>> [Tip] Campus traffic via VPN, other traffic via default "
       "route.\"\n";
  ss << "\n";
  ss << "exit 0\n";

  return ss.str();
}

bool write_script(const Config &cfg) {
  std::string path = utils::get_tunnel_path();
  std::string content = generate(cfg);

  for (const auto &server_ip : find_server_route_exceptions(cfg)) {
    logger::warn("Preserving upstream route for VPN server IP: " + server_ip);
  }

  if (!utils::write_file(path, content)) {
    utils::print_error("Failed to write tunnel script: " + path);
    logger::error("Failed to write tunnel script: " + path);
    return false;
  }

  // Set executable permission
  if (chmod(path.c_str(), 0755) != 0) {
    utils::print_error("Failed to set executable permission on: " + path);
    logger::error("Failed to chmod tunnel script: " + path);
    return false;
  }

  utils::sync_owner(path);

  logger::info("Tunnel script generated: " + path);
  return true;
}

} // namespace tunnel
} // namespace ecnuvpn
