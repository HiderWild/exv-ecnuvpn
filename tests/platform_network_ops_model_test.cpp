#include "platform/common/route_model.hpp"
#include "platform/common/dns_model.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <string>

namespace {

bool expect(bool condition, const char* message) {
    if (condition)
        return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

std::string read_text_file(const std::filesystem::path& path) {
    std::ifstream in(path);
    if (!in)
        throw std::runtime_error("failed to open " + path.string());
    return std::string(std::istreambuf_iterator<char>(in),
                       std::istreambuf_iterator<char>());
}

} // namespace

int main() {
    bool ok = true;

    // --- RouteTable: add / entries ---
    {
        exv::platform::RouteTable table;
        ok = expect(table.entries().empty(), "empty table should have no entries") && ok;

        exv::platform::RouteEntry r1{"10.0.0.0/8", "192.168.1.1", "eth0", 10, true};
        exv::platform::RouteEntry r2{"172.16.0.0/12", "192.168.1.1", "eth0", 20, true};
        table.add(r1);
        table.add(r2);

        ok = expect(table.entries().size() == 2, "table should have 2 entries after adds") && ok;
    }

    // --- RouteTable: find ---
    {
        exv::platform::RouteTable table;
        table.add({"10.0.0.0/8", "192.168.1.1", "eth0", 10, true});
        table.add({"172.16.0.0/12", "192.168.1.1", "eth0", 20, true});

        auto found = table.find("10.0.0.0/8");
        ok = expect(found.has_value(), "find should locate existing destination") && ok;
        ok = expect(found->gateway == "192.168.1.1", "found entry should match gateway") && ok;
        ok = expect(found->metric == 10, "found entry should match metric") && ok;

        auto missing = table.find("192.168.0.0/16");
        ok = expect(!missing.has_value(), "find should return nullopt for missing destination") && ok;
    }

    // --- RouteTable: remove ---
    {
        exv::platform::RouteTable table;
        table.add({"10.0.0.0/8", "192.168.1.1", "eth0", 10, true});
        table.add({"172.16.0.0/12", "192.168.1.1", "eth0", 20, true});

        bool removed = table.remove("10.0.0.0/8", "192.168.1.1");
        ok = expect(removed, "remove should return true for existing route") && ok;
        ok = expect(table.entries().size() == 1, "table should have 1 entry after removal") && ok;
        ok = expect(!table.find("10.0.0.0/8").has_value(), "removed route should not be findable") && ok;

        bool missing = table.remove("10.0.0.0/8", "192.168.1.1");
        ok = expect(!missing, "remove should return false for non-existing route") && ok;
    }

    // --- RouteTable: diff ---
    {
        exv::platform::RouteTable baseline;
        baseline.add({"10.0.0.0/8", "192.168.1.1", "eth0", 10, true});

        exv::platform::RouteTable snapshot;
        snapshot.add({"10.0.0.0/8", "192.168.1.1", "eth0", 10, true});
        snapshot.add({"172.16.0.0/12", "192.168.1.1", "eth0", 20, true});
        snapshot.add({"192.168.0.0/16", "10.0.0.1", "tun0", 5, true});

        auto extra = baseline.diff(snapshot);
        ok = expect(extra.size() == 2, "diff should find 2 routes in snapshot not in baseline") && ok;
        ok = expect(extra[0].destination == "172.16.0.0/12", "first diff entry should be 172.16.0.0/12") && ok;
        ok = expect(extra[1].destination == "192.168.0.0/16", "second diff entry should be 192.168.0.0/16") && ok;

        auto empty = snapshot.diff(baseline);
        ok = expect(empty.empty(), "diff from superset to subset should be empty") && ok;
    }

    // --- RouteTable: stubs ---
    {
        auto current = exv::platform::RouteTable::current();
        ok = expect(current.entries().empty(), "stub current() should return empty table") && ok;

        exv::platform::RouteTable table;
        ok = expect(table.apply_add({"10.0.0.0/8", "192.168.1.1", "eth0", 10, true}),
                    "stub apply_add should return true") && ok;
        ok = expect(table.apply_remove({"10.0.0.0/8", "192.168.1.1", "eth0", 10, true}),
                    "stub apply_remove should return true") && ok;
    }

    // --- DnsConfig: set / get settings ---
    {
        exv::platform::DnsConfig cfg;
        auto s0 = cfg.settings();
        ok = expect(s0.servers.empty(), "initial settings should have empty servers") && ok;
        ok = expect(s0.search_domain.empty(), "initial settings should have empty search_domain") && ok;
        ok = expect(s0.suffixes.empty(), "initial settings should have empty suffixes") && ok;

        cfg.set_servers({"8.8.8.8", "8.8.4.4"});
        cfg.set_search_domain("vpn.example.com");
        cfg.set_suffixes({"example.com", "corp.example.com"});

        auto s1 = cfg.settings();
        ok = expect(s1.servers.size() == 2, "settings should have 2 servers") && ok;
        ok = expect(s1.servers[0] == "8.8.8.8", "first server should be 8.8.8.8") && ok;
        ok = expect(s1.servers[1] == "8.8.4.4", "second server should be 8.8.4.4") && ok;
        ok = expect(s1.search_domain == "vpn.example.com", "search domain should match") && ok;
        ok = expect(s1.suffixes.size() == 2, "settings should have 2 suffixes") && ok;
        ok = expect(s1.suffixes[0] == "example.com", "first suffix should match") && ok;
    }

    // --- DnsConfig: overwrite ---
    {
        exv::platform::DnsConfig cfg;
        cfg.set_servers({"1.1.1.1"});
        cfg.set_servers({"9.9.9.9"});
        auto s = cfg.settings();
        ok = expect(s.servers.size() == 1, "overwritten servers should have 1 entry") && ok;
        ok = expect(s.servers[0] == "9.9.9.9", "overwritten server should be 9.9.9.9") && ok;
    }

    // --- DnsConfig: stubs ---
    {
        auto current = exv::platform::DnsConfig::current("eth0");
        ok = expect(current.servers.empty(), "stub current() should return empty settings") && ok;

        exv::platform::DnsConfig cfg;
        exv::platform::DnsSettings apply_settings{{"8.8.8.8"}, "vpn.example.com", {}, "eth0"};
        ok = expect(cfg.apply(apply_settings),
                    "stub apply should return true") && ok;
        ok = expect(cfg.restore("eth0"), "stub restore should return true") && ok;
    }

    // --- PlatformNetworkOps factory wiring ---
    {
        const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
        const auto source =
            read_text_file(source_dir / "src" / "platform" / "common" /
                           "platform_network_ops.cpp");
        ok = expect(source.find("platform/darwin/platform_network_ops_darwin.hpp") !=
                        std::string::npos,
                    "factory source should include the Darwin backend") &&
             ok;
        ok = expect(source.find("return create_darwin_platform_network_ops();") !=
                        std::string::npos,
                    "Darwin PlatformNetworkOps::create must return a real backend") &&
             ok;
        ok = expect(source.find("platform/linux/platform_network_ops_linux.hpp") !=
                        std::string::npos,
                    "factory source should include the Linux backend") &&
             ok;
        ok = expect(source.find("return create_linux_platform_network_ops();") !=
                        std::string::npos,
                    "Linux PlatformNetworkOps::create must return a real backend") &&
             ok;

        const auto cmake = read_text_file(source_dir / "CMakeLists.txt");
        ok = expect(cmake.find("src/platform/linux/platform_network_ops_linux.cpp") !=
                        std::string::npos,
                    "Linux PlatformNetworkOps backend must be in Linux platform sources") &&
             ok;
    }

    // --- Linux PlatformNetworkOps durable cleanup facts ---
    {
        const auto source_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR);
        const auto linux_source =
            read_text_file(source_dir / "src" / "platform" / "linux" /
                           "platform_network_ops_linux.cpp");

        ok = expect(linux_source.find("std::vector<ManagedNetworkResource> managed_resources() const override") !=
                        std::string::npos,
                    "Linux backend must export managed cleanup facts") &&
             ok;
        ok = expect(linux_source.find("\"adapter\"") != std::string::npos &&
                        linux_source.find("\"linux_interface_config\"") !=
                            std::string::npos &&
                        linux_source.find("\"linux_route\"") != std::string::npos &&
                        linux_source.find("\"linux_server_bypass_route\"") !=
                            std::string::npos &&
                        linux_source.find("\"linux_dns\"") != std::string::npos,
                    "Linux backend must export adapter, interface, route, bypass, and DNS facts") &&
             ok;
        ok = expect(linux_source.find("resource.type == \"linux_route\"") !=
                        std::string::npos,
                    "Linux cleanup must consume linux_route facts") &&
             ok;
        ok = expect(linux_source.find("resource.type == \"linux_server_bypass_route\"") !=
                        std::string::npos,
                    "Linux cleanup must consume linux_server_bypass_route facts") &&
             ok;
        ok = expect(linux_source.find("resource.type == \"linux_dns\"") !=
                        std::string::npos,
                    "Linux cleanup must consume linux_dns facts") &&
             ok;
        ok = expect(linux_source.find("resource.type == \"route\"") ==
                        std::string::npos,
                    "Linux cleanup must not depend on generic route resource strings") &&
             ok;

        const auto route_cleanup =
            linux_source.find("resource.type == \"linux_route\"");
        const auto dns_cleanup = linux_source.find("resource.type == \"linux_dns\"");
        const auto adapter_cleanup = linux_source.find("ip tuntap del dev ");
        ok = expect(route_cleanup != std::string::npos &&
                        dns_cleanup != std::string::npos &&
                        adapter_cleanup != std::string::npos &&
                        route_cleanup < dns_cleanup &&
                        dns_cleanup < adapter_cleanup,
                    "Linux cleanup order must be routes, DNS, then tunnel adapter") &&
             ok;
    }

    return ok ? 0 : 1;
}
