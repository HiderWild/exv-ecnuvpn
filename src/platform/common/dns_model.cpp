#include "dns_model.hpp"

namespace exv::platform {

void DnsConfig::set_servers(const std::vector<std::string>& servers) {
    settings_.servers = servers;
}

void DnsConfig::set_search_domain(const std::string& domain) {
    settings_.search_domain = domain;
}

void DnsConfig::set_suffixes(const std::vector<std::string>& suffixes) {
    settings_.suffixes = suffixes;
}

DnsSettings DnsConfig::settings() const {
    return settings_;
}

DnsSettings DnsConfig::current(const std::string& /*interface_name*/) {
    // Stub: returns empty settings. Platform implementation in Phase 3.
    return DnsSettings{};
}

bool DnsConfig::apply(const DnsSettings& /*settings*/) {
    // Stub: always succeeds. Platform implementation in Phase 3.
    return true;
}

bool DnsConfig::restore(const std::string& /*interface_name*/) {
    // Stub: always succeeds. Platform implementation in Phase 3.
    return true;
}

} // namespace exv::platform
