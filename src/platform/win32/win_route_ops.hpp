#pragma once

#include "platform/common/route_model.hpp"

#include <string>
#include <vector>

namespace exv::platform::win32 {

class WinRouteOps {
public:
  // Apply a route (add to OS routing table).
  static bool apply_route(const RouteEntry &route);

  // Remove a route. Idempotent: returns true if route is already gone.
  static bool remove_route(const RouteEntry &route);

  // Get current OS routing table.
  static RouteTable current_routes();

  // Apply multiple routes, returning the subset that were successfully added.
  static std::vector<RouteEntry>
  apply_routes(const std::vector<RouteEntry> &routes);

  // Remove routes that were added by this session (idempotent).
  static bool cleanup_routes(const std::vector<RouteEntry> &routes);

  // Change the metric of an existing route by destination.
  static bool set_route_metric(const std::string &destination, int metric);
};

} // namespace exv::platform::win32
