#pragma once
#include <string>
#include <vector>
#include <optional>

namespace exv::platform {

struct DnsSettings {
    std::vector<std::string> servers;
    std::string search_domain;
    std::vector<std::string> suffixes;
    std::string interface_name;
};

class DnsConfig {
public:
    void set_servers(const std::vector<std::string>& servers);
    void set_search_domain(const std::string& domain);
    void set_suffixes(const std::vector<std::string>& suffixes);

    DnsSettings settings() const;

    // Platform operations
    static DnsSettings current(const std::string& interface_name);
    bool apply(const DnsSettings& settings);
    bool restore(const std::string& interface_name);

private:
    DnsSettings settings_;
};

} // namespace exv::platform
