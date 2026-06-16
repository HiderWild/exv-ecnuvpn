// Tests for CleanupRegistry: resource tracking, persistence.

#include "helper/runtime/cleanup_registry.hpp"
#include "helper/common/helper_messages.hpp"
#include "core/lifecycle/core_paths.hpp"
#include "core/lifecycle/core_registry.hpp"

#include <iostream>
#include <string>
#include <filesystem>
#include <fstream>
#include <sstream>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
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

std::string read_file(const std::filesystem::path& path) {
    std::ifstream input(path);
    std::ostringstream contents;
    contents << input.rdbuf();
    return contents.str();
}

} // namespace

int main() {
    bool ok = true;

    using exv::helper::CleanupRegistry;
    using exv::helper::CleanupRecord;
    using exv::helper::CoreRegistryCleanupBinding;
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

        auto records = registry.all_records();
        ok = expect(records.size() == 1,
                    "add_resource should keep the registered record") && ok;
        ok = expect(records[0].managed_resources.size() == 1,
                    "add_resource should persist ManagedResource on CleanupRecord") && ok;
        ok = expect(records[0].managed_resources[0].type == "firewall_rule",
                    "stored managed resource type should match") && ok;
        ok = expect(records[0].managed_resources[0].detail == "ECNU-VPN-kill-switch",
                    "stored managed resource detail should match") && ok;
        ok = expect(records[0].firewall_rules.empty(),
                    "add_resource should not encode managed resources in firewall_rules") && ok;

        auto resources = registry.get_resources(rec.session_id);
        bool found = false;
        for (const auto& r : resources) {
            if (r.type == "firewall_rule" && r.detail == "ECNU-VPN-kill-switch") {
                found = true;
            }
        }
        ok = expect(found, "added firewall resource should be in get_resources") && ok;
    }

    // --- register_session preserves explicit managed resources ---
    {
        CleanupRegistry registry;
        CleanupRecord rec;
        rec.session_id.value = "ses-explicit";
        rec.managed_resources.push_back({"adapter", "ECNU-VPN"});
        rec.managed_resources.push_back({"route", "10.0.0.0/8"});

        registry.register_session(rec);

        auto records = registry.all_records();
        ok = expect(records.size() == 1,
                    "explicit managed resource record should be snapshotted") && ok;
        ok = expect(records[0].managed_resources.size() == 2,
                    "all_records should preserve explicit managed resources") && ok;

        auto resources = registry.get_resources(rec.session_id);
        ok = expect(resources.size() == 2,
                    "get_resources should return explicit managed resources") && ok;
        ok = expect(resources[0].type == "adapter" &&
                        resources[0].detail == "ECNU-VPN",
                    "first explicit managed resource should match") && ok;
        ok = expect(resources[1].type == "route" &&
                        resources[1].detail == "10.0.0.0/8",
                    "second explicit managed resource should match") && ok;
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
            registry.add_resource(rec.session_id, {"adapter", "ECNU-VPN"});
            registry.add_resource(rec.session_id, {"route", "10.0.0.0/8"});

            bool saved = registry.save_to_disk(path);
            ok = expect(saved, "save_to_disk should succeed") && ok;
            ok = expect(fs::exists(path), "registry file should exist") && ok;

            const auto saved_json = read_file(path);
            ok = expect(saved_json.find("\"managed_resources\"") != std::string::npos,
                        "registry file should persist managed_resources") && ok;
            ok = expect(saved_json.find("__managed__") == std::string::npos,
                        "registry file should not contain managed resource side-channel") && ok;
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
            ok = expect(records[0].managed_resources.size() == 2,
                        "loaded managed_resources should have 2 entries") && ok;
            ok = expect(records[0].managed_resources[0].type == "adapter" &&
                            records[0].managed_resources[0].detail == "ECNU-VPN",
                        "loaded adapter managed_resource should match") && ok;
            ok = expect(records[0].managed_resources[1].type == "route" &&
                            records[0].managed_resources[1].detail == "10.0.0.0/8",
                        "loaded route managed_resource should match") && ok;

            auto resources = registry.get_resources(records[0].session_id);
            ok = expect(resources.size() == 5,
                        "loaded get_resources should include legacy fields and explicit managed resources") && ok;
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

    // --- matching core registry binding is compare-and-deleted on session removal ---
    {
        namespace fs = std::filesystem;
        const auto root = fs::temp_directory_path() /
            ("ecnuvpn-helper-core-registry-match-" + std::to_string(current_process_id()));
        std::error_code ec;
        fs::remove_all(root, ec);
        fs::create_directories(root, ec);

        CleanupRegistry registry;
        CleanupRecord rec;
        rec.session_id.value = "ses-core-match";
        registry.register_session(rec);

        exv::core::lifecycle::CoreRegistrySnapshot snapshot;
        snapshot.core_instance_id = "core-instance-helper";
        snapshot.pid = 5150;
        snapshot.core_path = "C:/Program Files/ECNU-VPN/exv.exe";
        snapshot.ipc_path = exv::core::lifecycle::core_ipc_path(root.string());
        snapshot.ipc_protocol_version = "ipc-v1";
        snapshot.app_version = "3.3.0";
        snapshot.contract_version = "2026-06-16.cli-core-ui-contract.v1";
        snapshot.started_at = "2026-06-16T12:00:00.000Z";
        snapshot.last_heartbeat_at = "2026-06-16T12:00:01.000Z";
        snapshot.last_known_tunnel_phase = "idle";
        snapshot.last_known_connected = false;
        snapshot.last_known_network_ready = false;
        snapshot.helper_core_lease_id = "core-lease-match";

        const auto registry_path =
            exv::core::lifecycle::core_registry_path(root.string());
        ok = expect(exv::core::lifecycle::write_core_registry(snapshot, registry_path),
                    "matching helper cleanup test should write core registry") && ok;

        CoreRegistryCleanupBinding binding;
        binding.registry_path = registry_path;
        binding.delete_match =
            exv::core::lifecycle::core_registry_delete_match(snapshot);
        registry.bind_core_registry_cleanup(rec.session_id, binding);

        registry.remove_session(rec.session_id);
        ok = expect(!fs::exists(registry_path),
                    "matching helper cleanup should delete versioned core registry") && ok;

        fs::remove_all(root, ec);
    }

    // --- mismatched core registry binding must not delete versioned registry ---
    {
        namespace fs = std::filesystem;
        const auto root = fs::temp_directory_path() /
            ("ecnuvpn-helper-core-registry-mismatch-" + std::to_string(current_process_id()));
        std::error_code ec;
        fs::remove_all(root, ec);
        fs::create_directories(root, ec);

        CleanupRegistry registry;
        CleanupRecord rec;
        rec.session_id.value = "ses-core-mismatch";
        registry.register_session(rec);

        exv::core::lifecycle::CoreRegistrySnapshot snapshot;
        snapshot.core_instance_id = "core-instance-helper";
        snapshot.pid = 6160;
        snapshot.core_path = "C:/Program Files/ECNU-VPN/exv.exe";
        snapshot.ipc_path = exv::core::lifecycle::core_ipc_path(root.string());
        snapshot.ipc_protocol_version = "ipc-v1";
        snapshot.app_version = "3.3.0";
        snapshot.contract_version = "2026-06-16.cli-core-ui-contract.v1";
        snapshot.started_at = "2026-06-16T12:00:00.000Z";
        snapshot.last_heartbeat_at = "2026-06-16T12:00:01.000Z";
        snapshot.last_known_tunnel_phase = "idle";
        snapshot.last_known_connected = false;
        snapshot.last_known_network_ready = false;
        snapshot.helper_core_lease_id = "core-lease-actual";

        const auto registry_path =
            exv::core::lifecycle::core_registry_path(root.string());
        ok = expect(exv::core::lifecycle::write_core_registry(snapshot, registry_path),
                    "mismatch helper cleanup test should write core registry") && ok;

        CoreRegistryCleanupBinding binding;
        binding.registry_path = registry_path;
        binding.delete_match =
            exv::core::lifecycle::core_registry_delete_match(snapshot);
        binding.delete_match.helper_core_lease_id = "core-lease-other";
        registry.bind_core_registry_cleanup(rec.session_id, binding);

        ok = expect(fs::exists(registry_path),
                    "binding setup alone must not delete core registry") && ok;

        registry.remove_session(rec.session_id);
        ok = expect(fs::exists(registry_path),
                    "mismatched helper cleanup must keep versioned core registry") && ok;

        fs::remove_all(root, ec);
    }

    if (ok) {
        std::cout << "helper_cleanup_registry_test: all assertions passed\n";
    } else {
        std::cerr << "helper_cleanup_registry_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
