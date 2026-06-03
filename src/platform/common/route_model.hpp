#pragma once
#include <string>
#include <vector>
#include <optional>

namespace exv::platform {

struct RouteEntry {
    std::string destination;
    std::string gateway;
    std::string interface_name;
    int metric = 0;
    bool is_active = true;
};

class RouteTable {
public:
    void add(const RouteEntry& route);
    bool remove(const std::string& destination, const std::string& gateway);
    std::vector<RouteEntry> entries() const;
    std::optional<RouteEntry> find(const std::string& destination) const;

    // Diff for cleanup
    std::vector<RouteEntry> diff(const RouteTable& other) const;

    // Platform operations
    static RouteTable current();
    bool apply_add(const RouteEntry& route);
    bool apply_remove(const RouteEntry& route);

private:
    std::vector<RouteEntry> routes_;
};

} // namespace exv::platform
