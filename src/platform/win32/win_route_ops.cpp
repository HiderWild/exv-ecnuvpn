#include "win_route_ops.hpp"

#ifdef _WIN32

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <iphlpapi.h>

#include <cstdint>
#include <limits>
#include <string>
#include <vector>

#pragma comment(lib, "iphlpapi.lib")
#pragma comment(lib, "ws2_32.lib")

namespace exv::platform::win32 {

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// Convert dotted-decimal IPv4 string to DWORD in network byte order.
// Returns 0 on failure (same behaviour as inet_addr, but uses the modern API).
static DWORD ip_to_dword(const std::string &ip) {
  IN_ADDR addr{};
  if (InetPtonA(AF_INET, ip.c_str(), &addr) != 1)
    return 0;
  return addr.s_addr;
}

// Convert a DWORD in network byte order to a dotted-decimal string.
static std::string dword_to_ip(DWORD value) {
  IN_ADDR addr{};
  addr.s_addr = value;
  char buf[INET_ADDRSTRLEN] = {0};
  if (!InetNtopA(AF_INET, &addr, buf, sizeof(buf)))
    return {};
  return std::string(buf);
}

// Build a subnet mask in network byte order from a prefix length (0-32).
static DWORD prefix_to_mask(int prefix_len) {
  if (prefix_len <= 0)
    return 0;
  if (prefix_len >= 32)
    return 0xFFFFFFFF;
  // Host-order mask, then convert to network order for the Win32 struct.
  std::uint32_t host_mask =
      std::numeric_limits<std::uint32_t>::max() << (32 - prefix_len);
  return htonl(host_mask);
}

// Count the leading 1-bits in a network-byte-order mask to recover the prefix
// length.  Returns 0 for 0.0.0.0 and 32 for 255.255.255.255.
static int mask_to_prefix(DWORD mask) {
  if (mask == 0)
    return 0;
  std::uint32_t host = ntohl(mask);
  int prefix = 0;
  while (host & 0x80000000u) {
    ++prefix;
    host <<= 1;
  }
  return prefix;
}

// Parse a destination string that may be either "ip" or "ip/prefix".
// On success, fills |ip| (network order) and |mask| (network order).
static bool parse_destination(const std::string &dest, DWORD &ip,
                              DWORD &mask) {
  std::size_t slash = dest.find('/');
  if (slash == std::string::npos) {
    ip = ip_to_dword(dest);
    if (ip == 0 && dest != "0.0.0.0")
      return false;
    mask = 0xFFFFFFFF; // host route
    return true;
  }

  std::string ip_str = dest.substr(0, slash);
  int prefix = 0;
  try {
    prefix = std::stoi(dest.substr(slash + 1));
  } catch (...) {
    return false;
  }
  if (prefix < 0 || prefix > 32)
    return false;

  ip = ip_to_dword(ip_str);
  if (ip == 0 && ip_str != "0.0.0.0")
    return false;
  mask = prefix_to_mask(prefix);
  return true;
}

// ---------------------------------------------------------------------------
// WinRouteOps
// ---------------------------------------------------------------------------

bool WinRouteOps::apply_route(const RouteEntry &route) {
  MIB_IPFORWARDROW row = {};

  DWORD dest_ip = 0, dest_mask = 0;
  if (!parse_destination(route.destination, dest_ip, dest_mask))
    return false;

  row.dwForwardDest = dest_ip;
  row.dwForwardMask = dest_mask;
  row.dwForwardNextHop = ip_to_dword(route.gateway);
  row.dwForwardMetric1 = static_cast<DWORD>(route.metric);
  row.dwForwardProto = MIB_IPPROTO_NETMGMT;

  DWORD result = CreateIpForwardEntry(&row);
  return (result == NO_ERROR);
}

bool WinRouteOps::remove_route(const RouteEntry &route) {
  MIB_IPFORWARDROW row = {};

  DWORD dest_ip = 0, dest_mask = 0;
  if (!parse_destination(route.destination, dest_ip, dest_mask))
    return false;

  row.dwForwardDest = dest_ip;
  row.dwForwardMask = dest_mask;
  row.dwForwardNextHop = ip_to_dword(route.gateway);

  DWORD result = DeleteIpForwardEntry(&row);
  // ERROR_NOT_FOUND means the route is already gone -- treat as success so
  // that cleanup_routes() is idempotent.
  return (result == NO_ERROR || result == ERROR_NOT_FOUND);
}

RouteTable WinRouteOps::current_routes() {
  RouteTable table;

  ULONG size = 0;
  GetIpForwardTable(nullptr, &size, FALSE);

  std::vector<BYTE> buffer(size);
  auto *fwd_table = reinterpret_cast<PMIB_IPFORWARDTABLE>(buffer.data());

  if (GetIpForwardTable(fwd_table, &size, FALSE) != NO_ERROR)
    return table;

  for (DWORD i = 0; i < fwd_table->dwNumEntries; i++) {
    const auto &row = fwd_table->table[i];
    RouteEntry entry;

    entry.destination = dword_to_ip(row.dwForwardDest);
    int prefix = mask_to_prefix(row.dwForwardMask);
    entry.destination += "/" + std::to_string(prefix);

    entry.gateway = dword_to_ip(row.dwForwardNextHop);
    entry.metric = static_cast<int>(row.dwForwardMetric1);
    entry.is_active = true;

    table.add(entry);
  }

  return table;
}

std::vector<RouteEntry>
WinRouteOps::apply_routes(const std::vector<RouteEntry> &routes) {
  std::vector<RouteEntry> applied;
  applied.reserve(routes.size());
  for (const auto &route : routes) {
    if (apply_route(route))
      applied.push_back(route);
  }
  return applied;
}

bool WinRouteOps::cleanup_routes(const std::vector<RouteEntry> &routes) {
  bool all_ok = true;
  for (const auto &route : routes) {
    if (!remove_route(route))
      all_ok = false;
  }
  return all_ok;
}

bool WinRouteOps::set_route_metric(const std::string &destination, int metric) {
  // Retrieve the full routing table and find the matching row.
  ULONG size = 0;
  GetIpForwardTable(nullptr, &size, FALSE);

  std::vector<BYTE> buffer(size);
  auto *fwd_table = reinterpret_cast<PMIB_IPFORWARDTABLE>(buffer.data());

  if (GetIpForwardTable(fwd_table, &size, FALSE) != NO_ERROR)
    return false;

  DWORD dest_ip = 0, dest_mask = 0;
  if (!parse_destination(destination, dest_ip, dest_mask))
    return false;

  for (DWORD i = 0; i < fwd_table->dwNumEntries; i++) {
    auto &row = fwd_table->table[i];
    if (row.dwForwardDest == dest_ip && row.dwForwardMask == dest_mask) {
      row.dwForwardMetric1 = static_cast<DWORD>(metric);
      DWORD result = SetIpForwardEntry(&row);
      return (result == NO_ERROR);
    }
  }

  return false; // route not found
}

} // namespace exv::platform::win32

#else
// ---------------------------------------------------------------------------
// Non-Windows unavailable fallback for a Windows-only feature.
// ---------------------------------------------------------------------------

namespace exv::platform::win32 {

bool WinRouteOps::apply_route(const RouteEntry &) { return false; }
bool WinRouteOps::remove_route(const RouteEntry &) { return false; }
RouteTable WinRouteOps::current_routes() { return {}; }
std::vector<RouteEntry>
WinRouteOps::apply_routes(const std::vector<RouteEntry> &) {
  return {};
}
bool WinRouteOps::cleanup_routes(const std::vector<RouteEntry> &) {
  return false;
}
bool WinRouteOps::set_route_metric(const std::string &, int) { return false; }

} // namespace exv::platform::win32

#endif // _WIN32
