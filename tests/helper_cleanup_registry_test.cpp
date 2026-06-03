// Tests for CleanupRegistry: resource tracking, persistence.

#include "helper_runtime/cleanup_registry.hpp"
#include "helper_common/helper_messages.hpp"

#include <iostream>
#include <string>
#include <filesystem>

#ifdef _WIN32
#include <windows.h>
#endif

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

int current_process_id() {
#ifdef _WIN32
    return static_cast<int>(GetCurrentProcessId());
#else
    return static_cast<int>(getpid());
#endif
}

} // namespace

int main() {
    bool ok = true;

    using exv::helper::CleanupRegistry;
    using exv::helper::CleanupRecord;
    using exv::helper::ManagedResource;
    using exv::helper::SessionId;

    // --- register_session and all_records ---
    {
        CleanupRegistry registry;
        CleanupRecord rec;
        rec.session_id.value = "ses-1";
        rec.adapter_name = "ECNU-VPN";
        rec.routes.push_back({"0.0.0.0/0", "10.0.0.1", 0});
        rec.dns.servers = {"8.8.8.8"};

        registry.register_session(rec);

        auto records = registry.all_records();
        ok = expect(records.size() == 1,
                    "should have 1 record after registration") && ok;
        ok = expect(records[0].session_id.value == "ses-1",
                    "record session_id should match") && ok;
        ok = expect(records[0].adapter_name == "ECNU-VPN",
                    "record adapter_name should match") && ok;
    }

    // --- get_resources returns routes and DNS ---
    {
        CleanupRegistry registry;
        CleanupRecord rec;
        rec.session_id.value = "ses-1";
        rec.adapter_name = "ECNU-VPN";
        rec.routes.push_back({"0.0.0.0/0", "10.0.0.1", 0});
        rec.routes.push_back({"10.0.0.0/8", "10.0.0.1", 0});
        rec.dns.servers = {"8.8.8.8", "8.8.4.4"};

        registry.register_session(rec);
        auto resources = registry.get_resources(rec.session_id);

        ok = expect(resources.size() >= 4,
                    "should have at least 4 resources") && ok;

        int route_count = 0;
        int dns_count = 0;
        int adapter_count = 0;
        for (const auto& r : resources) {
            if (r.type == "route") route_count++;
            if (r.type == "dns") dns_count++;
            if (r.type == "adapter") adapter_count++;
        }
        ok = expect(route_count == 2, "should have 2 route resources") && ok;
        ok = expect(dns_count == 2, "should have 2 dns resources") && ok;
        ok = expect(adapter_count == 1, "should have 1 adapter resource") && ok;
    }

    // --- add_resource adds a managed resource ---
    {
        CleanupRegistry registry;
        CleanupRecord rec;
        rec.session_id.value = "ses-1";
        registry.register_session(rec);

        ManagedResource res;
        res.type = "firewall_rule";
        res.detail = "ECNU-VPN-kill-switch";
        registry.add_resource(rec.session_id, res);

        auto resources = registry.get_resources(rec.session_id);
        bool found = false;
        for (const auto& r : resources) {
            if (r.type == "firewall_rule" && r.detail == "ECNU-VPN-kill-switch") {
                found = true;
            }
        }
        ok = expect(found, "added firewall resource should be in get_resources") && ok;
    }

    // --- add_resource on unknown session is a no-op ---
    {
        CleanupRegistry registry;
        SessionId unknown;
        unknown.value = "nonexistent";
        ManagedResource res{"route", "0.0.0.0/0"};
        registry.add_resource(unknown, res);

        auto resources = registry.get_resources(unknown);
        ok = expect(resources.empty(),
                    "unknown session should have no resources") && ok;
    }

    // --- remove_session removes the record ---
    {
        CleanupRegistry registry;
        CleanupRecord rec;
        rec.session_id.value = "ses-1";
        registry.register_session(rec);

        ok = expect(registry.all_records().size() == 1,
                    "should have 1 record before removal") && ok;

        registry.remove_session(rec.session_id);
        ok = expect(registry.all_records().empty(),
                    "should have 0 records after removal") && ok;
    }

    // --- get_resources for unknown session returns empty ---
    {
        CleanupRegistry registry;
        SessionId unknown;
        unknown.value = "nonexistent";
        auto resources = registry.get_resources(unknown);
        ok = expect(resources.empty(),
                    "unknown session should have empty resources") && ok;
    }

    // --- save_to_disk and load_from_disk ---
    {
        namespace fs = std::filesystem;
        const auto root = fs::temp_directory_path() /
            ("ecnuvpn-cleanup-registry-test-" + std::to_string(current_process_id()));
        fs::create_directories(root);
        const auto path = (root / "registry.json").string();

        {
            CleanupRegistry registry;
            CleanupRecord rec;
            rec.session_id.value = "ses-1";
            rec.adapter_name = "ECNU-VPN";
            rec.routes.push_back({"0.0.0.0/0", "10.0.0.1", 0});
            rec.dns.servers = {"8.8.8.8"};
            registry.register_session(rec);

            bool saved = registry.save_to_disk(path);
            ok = expect(saved, "save_to_disk should succeed") && ok;
            ok = expect(fs::exists(path), "registry file should exist") && ok;
        }

        {
            CleanupRegistry registry;
            bool loaded = registry.load_from_disk(path);
            ok = expect(loaded, "load_from_disk should succeed") && ok;

            auto records = registry.all_records();
            ok = expect(records.size() == 1,
                        "loaded registry should have 1 record") && ok;
            ok = expect(records[0].session_id.value == "ses-1",
                        "loaded session_id should match") && ok;
            ok = expect(records[0].adapter_name == "ECNU-VPN",
                        "loaded adapter_name should match") && ok;
            ok = expect(records[0].routes.size() == 1,
                        "loaded routes should have 1 entry") && ok;
            ok = expect(records[0].dns.servers.size() == 1,
                        "loaded DNS should have 1 server") && ok;
        }

        fs::remove_all(root);
    }

    // --- save_to_disk to invalid path returns false ---
    {
        CleanupRegistry registry;
        bool saved = registry.save_to_disk("/nonexistent/path/registry.json");
        ok = expect(!saved, "save to invalid path should return false") && ok;
    }

    // --- load_from_disk from nonexistent file returns false ---
    {
        CleanupRegistry registry;
        bool loaded = registry.load_from_disk("/nonexistent/path/registry.json");
        ok = expect(!loaded, "load from nonexistent file should return false") && ok;
    }

    // --- Multiple sessions ---
    {
        CleanupRegistry registry;

        CleanupRecord r1;
        r1.session_id.value = "ses-1";
        r1.adapter_name = "adapter-1";
        registry.register_session(r1);

        CleanupRecord r2;
        r2.session_id.value = "ses-2";
        r2.adapter_name = "adapter-2";
        registry.register_session(r2);

        ok = expect(registry.all_records().size() == 2,
                    "should have 2 records") && ok;

        registry.remove_session(r1.session_id);
        ok = expect(registry.all_records().size() == 1,
                    "should have 1 record after removing one") && ok;
    }

    if (ok) {
        std::cout << "helper_cleanup_registry_test: all assertions passed\n";
    } else {
        std::cerr << "helper_cleanup_registry_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
