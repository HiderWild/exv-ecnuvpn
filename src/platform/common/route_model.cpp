#include "route_model.hpp"

#include <algorithm>

namespace exv::platform {

void RouteTable::add(const RouteEntry& route) {
    routes_.push_back(route);
}

bool RouteTable::remove(const std::string& destination, const std::string& gateway) {
    auto it = std::remove_if(routes_.begin(), routes_.end(),
        [&](const RouteEntry& r) {
            return r.destination == destination && r.gateway == gateway;
        });
    if (it == routes_.end())
        return false;
    routes_.erase(it, routes_.end());
    return true;
}

std::vector<RouteEntry> RouteTable::entries() const {
    return routes_;
}

std::optional<RouteEntry> RouteTable::find(const std::string& destination) const {
    for (const auto& r : routes_) {
        if (r.destination == destination)
            return r;
    }
    return std::nullopt;
}

std::vector<RouteEntry> RouteTable::diff(const RouteTable& other) const {
    std::vector<RouteEntry> result;
    for (const auto& r : other.routes_) {
        bool found = false;
        for (const auto& own : routes_) {
            if (own.destination == r.destination && own.gateway == r.gateway) {
                found = true;
                break;
            }
        }
        if (!found)
            result.push_back(r);
    }
    return result;
}

RouteTable RouteTable::current() {
    // Model-only fallback; platform-specific code supplies live route tables.
    return RouteTable{};
}

bool RouteTable::apply_add(const RouteEntry& /*route*/) {
    // Model-only fallback used by value-semantics tests.
    return true;
}

bool RouteTable::apply_remove(const RouteEntry& /*route*/) {
    // Model-only fallback used by value-semantics tests.
    return true;
}

} // namespace exv::platform
