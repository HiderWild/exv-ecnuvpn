// Tests for CleanupRegistry: resource tracking, persistence.

#include "helper/helper_handler.hpp"
#include "helper/runtime/cleanup_registry.hpp"
#include "helper/common/helper_messages.hpp"
#include "core/lifecycle/core_paths.hpp"
#include "core/lifecycle/core_registry.hpp"
#include "runtime/runtime_context.hpp"

#include <nlohmann/json.hpp>

#include <atomic>
#include <chrono>
#include <iostream>
#include <string>
#include <filesystem>
#include <functional>
#include <fstream>
#include <sstream>
#include <thread>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif

namespace {

using json = nlohmann::json;

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

exv::helper::HelperResponse dispatch_json(exv::helper::HelperHandler& handler,
                                          exv::helper::HelperOp op,
                                          const json& payload) {
    exv::helper::HelperRequest request;
    request.op = op;
    request.payload_json = payload.dump();
    return handler.handle(request);
}

exv::core::lifecycle::CoreRegistrySnapshot make_core_registry_snapshot(
    const std::filesystem::path& root, int pid, const std::string& lease_id) {
    exv::core::lifecycle::CoreRegistrySnapshot snapshot;
    snapshot.core_instance_id = "core-instance-helper";
    snapshot.pid = pid;
    snapshot.core_path = "C:/Program Files/ECNU-VPN/exv.exe";
    snapshot.ipc_path = exv::core::lifecycle::core_ipc_path(root.string());
    snapshot.ipc_protocol_version = exv::core::lifecycle::ipc_protocol_name();
    snapshot.app_version = "3.3.0";
    snapshot.contract_version = "2026-06-16.cli-core-ui-contract.v1";
    snapshot.started_at = "2026-06-16T12:00:00.000Z";
    snapshot.last_heartbeat_at = "2026-06-16T12:00:01.000Z";
    snapshot.last_known_tunnel_phase = "idle";
    snapshot.last_known_connected = false;
    snapshot.last_known_network_ready = false;
    snapshot.helper_core_lease_id = lease_id;
    return snapshot;
}

} // namespace

namespace exv::core::lifecycle::testing {
using CoreRegistryCompareDeleteHook =
    std::function<void(const std::string& final_path,
                       const std::string& tombstone_path)>;

void set_compare_delete_quarantine_hook(
    CoreRegistryCompareDeleteHook hook);
} // namespace exv::core::lifecycle::testing

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

    // --- remove_session only removes the record; it must not cleanup core registry ---
    {
        namespace fs = std::filesystem;
        const auto root = fs::temp_directory_path() /
            ("ecnuvpn-helper-core-registry-remove-only-" +
             std::to_string(current_process_id()));
        std::error_code ec;
        fs::remove_all(root, ec);
        fs::create_directories(root, ec);

        CleanupRegistry registry;
        CleanupRecord rec;
        rec.session_id.value = "ses-core-remove-only";
        registry.register_session(rec);

        const auto snapshot =
            make_core_registry_snapshot(root, 4141, "core-lease-remove-only");
        const auto registry_path =
            exv::core::lifecycle::core_registry_path(root.string());
        ok = expect(exv::core::lifecycle::write_core_registry(snapshot,
                                                              registry_path),
                    "remove-only helper test should write core registry") && ok;

        CoreRegistryCleanupBinding binding;
        binding.registry_path = registry_path;
        binding.delete_match =
            exv::core::lifecycle::core_registry_delete_match(snapshot);
        registry.bind_core_registry_cleanup(rec.session_id, binding);

        registry.remove_session(rec.session_id);
        ok = expect(registry.all_records().empty(),
                    "remove_session should remove the cleanup record") && ok;
        ok = expect(fs::exists(registry_path),
                    "remove_session must not delete the versioned core registry") &&
             ok;

        fs::remove_all(root, ec);
    }

    // --- matching core registry binding is compare-and-deleted after successful cleanup ---
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

        CoreRegistryCleanupBinding stored_binding;
        stored_binding.registry_path = registry_path;
        stored_binding.delete_match =
            exv::core::lifecycle::core_registry_delete_match(snapshot);
        registry.bind_core_registry_cleanup(rec.session_id, stored_binding);

        const auto cleanup_binding =
            registry.core_registry_cleanup_binding(rec.session_id);
        ok = expect(cleanup_binding.has_value(),
                    "successful helper cleanup should expose registry cleanup binding") &&
             ok;
        if (cleanup_binding.has_value()) {
            ok = expect(
                     exv::core::lifecycle::compare_and_delete_core_registry(
                         cleanup_binding->registry_path,
                         cleanup_binding->delete_match),
                     "successful helper cleanup should compare-delete versioned core registry") &&
                 ok;
        }
        registry.remove_session(rec.session_id);
        ok = expect(!fs::exists(registry_path),
                    "successful helper cleanup should delete versioned core registry") && ok;

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

    // --- production helper cleanup binds and removes the versioned core registry ---
    {
        namespace fs = std::filesystem;
        const auto root = fs::temp_directory_path() /
            ("ecnuvpn-helper-core-registry-production-" +
             std::to_string(current_process_id()));
        std::error_code ec;
        fs::remove_all(root, ec);
        fs::create_directories(root, ec);

        ecnuvpn::runtime::bootstrap(root.string(), root.string(), true);

        exv::helper::HelperHandler handler;

        exv::helper::AcquireCoreLeaseRequest acquire_req;
        acquire_req.core_pid = 5150;
        acquire_req.purpose = "connect";
        auto acquire = dispatch_json(
            handler, exv::helper::HelperOp::AcquireCoreLease, json(acquire_req));
        ok = expect(acquire.success,
                    "AcquireCoreLease should succeed before production cleanup test") &&
             ok;
        const auto acquire_resp =
            exv::helper::acquire_core_lease_response_from_json(
                json::parse(acquire.payload_json));

        exv::helper::StartSessionRequest start_req;
        start_req.profile_id.value = "profile-helper";
        auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                                   json(start_req));
        ok = expect(start.success,
                    "StartSession should succeed before production cleanup test") &&
             ok;
        const auto start_resp = exv::helper::start_session_response_from_json(
            json::parse(start.payload_json));

        const auto registry_path = exv::core::lifecycle::core_registry_path(root.string());
        const auto snapshot =
            make_core_registry_snapshot(root, acquire_req.core_pid,
                                        acquire_resp.lease_id);
        ok = expect(exv::core::lifecycle::write_core_registry(snapshot, registry_path),
                    "production helper cleanup test should write versioned registry") &&
             ok;

        exv::helper::HeartbeatRequest heartbeat_req;
        heartbeat_req.session_id = start_resp.session_id;
        heartbeat_req.core_phase = "connected";
        auto heartbeat = dispatch_json(handler, exv::helper::HelperOp::Heartbeat,
                                       json(heartbeat_req));
        ok = expect(heartbeat.success,
                    "Heartbeat should succeed before production cleanup test") &&
             ok;

        const auto records = handler.cleanup_registry().all_records();
        ok = expect(records.size() == 1,
                    "production helper cleanup test should keep one cleanup record") &&
             ok;
        ok = expect(records.size() == 1 &&
                        records[0].core_registry_cleanup.has_value(),
                    "Heartbeat should bind core registry cleanup on the production path") &&
             ok;

        exv::helper::CleanupRequest cleanup_req;
        cleanup_req.session_id = start_resp.session_id;
        auto cleanup = dispatch_json(handler, exv::helper::HelperOp::Cleanup,
                                     json(cleanup_req));
        ok = expect(cleanup.success,
                    "production helper cleanup should succeed without managed resources") &&
             ok;
        ok = expect(!fs::exists(registry_path),
                    "successful production helper cleanup should delete the versioned core registry") &&
             ok;

        fs::remove_all(root, ec);
    }

    // --- production cleanup keeps record and lease when registry delete fails, then retries ---
    {
        namespace fs = std::filesystem;
        const auto root = fs::temp_directory_path() /
            ("ecnuvpn-helper-core-registry-delete-retry-" +
             std::to_string(current_process_id()));
        std::error_code ec;
        fs::remove_all(root, ec);
        fs::create_directories(root, ec);

        ecnuvpn::runtime::bootstrap(root.string(), root.string(), true);

        exv::helper::HelperHandler handler;

        exv::helper::AcquireCoreLeaseRequest acquire_req;
        acquire_req.core_pid = 8181;
        acquire_req.purpose = "connect";
        auto acquire = dispatch_json(
            handler, exv::helper::HelperOp::AcquireCoreLease, json(acquire_req));
        ok = expect(acquire.success,
                    "AcquireCoreLease should succeed before cleanup retry test") &&
             ok;
        const auto acquire_resp =
            exv::helper::acquire_core_lease_response_from_json(
                json::parse(acquire.payload_json));

        exv::helper::StartSessionRequest start_req;
        start_req.profile_id.value = "profile-helper-retry";
        auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                                   json(start_req));
        ok = expect(start.success,
                    "StartSession should succeed before cleanup retry test") && ok;
        const auto start_resp = exv::helper::start_session_response_from_json(
            json::parse(start.payload_json));

        const auto registry_path =
            exv::core::lifecycle::core_registry_path(root.string());
        const auto snapshot =
            make_core_registry_snapshot(root, acquire_req.core_pid,
                                        acquire_resp.lease_id);
        ok = expect(exv::core::lifecycle::write_core_registry(snapshot,
                                                              registry_path),
                    "cleanup retry test should write versioned registry") && ok;

        exv::helper::HeartbeatRequest heartbeat_req;
        heartbeat_req.session_id = start_resp.session_id;
        heartbeat_req.core_phase = "connected";
        auto heartbeat = dispatch_json(handler, exv::helper::HelperOp::Heartbeat,
                                       json(heartbeat_req));
        ok = expect(heartbeat.success,
                    "Heartbeat should bind registry cleanup before retry test") &&
             ok;

        fs::remove(registry_path, ec);

        exv::helper::CleanupRequest cleanup_req;
        cleanup_req.session_id = start_resp.session_id;
        auto failed_cleanup = dispatch_json(
            handler, exv::helper::HelperOp::Cleanup, json(cleanup_req));
        ok = expect(!failed_cleanup.success,
                    "cleanup should fail when registry compare-delete cannot run") &&
             ok;
        ok = expect(!handler.cleanup_registry().all_records().empty(),
                    "registry delete failure must keep cleanup record for retry") &&
             ok;
        ok = expect(handler.lease_manager().has_session(start_resp.session_id),
                    "registry delete failure must keep session lease for retry") &&
             ok;

        ok = expect(exv::core::lifecycle::write_core_registry(snapshot,
                                                              registry_path),
                    "cleanup retry test should restore versioned registry") && ok;

        auto retry_cleanup = dispatch_json(
            handler, exv::helper::HelperOp::Cleanup, json(cleanup_req));
        ok = expect(retry_cleanup.success,
                    "cleanup retry should succeed after registry is restorable") && ok;
        ok = expect(handler.cleanup_registry().all_records().empty(),
                    "successful cleanup retry should remove cleanup record") && ok;
        ok = expect(!handler.lease_manager().has_session(start_resp.session_id),
                    "successful cleanup retry should remove session lease") && ok;
        ok = expect(!fs::exists(registry_path),
                    "successful cleanup retry should delete versioned registry") && ok;

        fs::remove_all(root, ec);
    }

    // --- production cleanup must not hold state_mutex while deleting registry ---
    {
        namespace fs = std::filesystem;
        const auto root = fs::temp_directory_path() /
            ("ecnuvpn-helper-core-registry-delete-unlocked-" +
             std::to_string(current_process_id()));
        std::error_code ec;
        fs::remove_all(root, ec);
        fs::create_directories(root, ec);

        ecnuvpn::runtime::bootstrap(root.string(), root.string(), true);

        exv::helper::HelperHandler handler;

        exv::helper::AcquireCoreLeaseRequest acquire_req;
        acquire_req.core_pid = 8282;
        acquire_req.purpose = "connect";
        auto acquire = dispatch_json(
            handler, exv::helper::HelperOp::AcquireCoreLease, json(acquire_req));
        ok = expect(acquire.success,
                    "AcquireCoreLease should succeed before unlocked cleanup test") &&
             ok;
        const auto acquire_resp =
            exv::helper::acquire_core_lease_response_from_json(
                json::parse(acquire.payload_json));

        exv::helper::StartSessionRequest start_req;
        start_req.profile_id.value = "profile-helper-unlocked";
        auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                                   json(start_req));
        ok = expect(start.success,
                    "StartSession should succeed before unlocked cleanup test") &&
             ok;
        const auto start_resp = exv::helper::start_session_response_from_json(
            json::parse(start.payload_json));

        const auto registry_path =
            exv::core::lifecycle::core_registry_path(root.string());
        const auto snapshot =
            make_core_registry_snapshot(root, acquire_req.core_pid,
                                        acquire_resp.lease_id);
        ok = expect(exv::core::lifecycle::write_core_registry(snapshot,
                                                              registry_path),
                    "unlocked cleanup test should write versioned registry") &&
             ok;

        exv::helper::HeartbeatRequest heartbeat_req;
        heartbeat_req.session_id = start_resp.session_id;
        heartbeat_req.core_phase = "connected";
        auto heartbeat = dispatch_json(handler, exv::helper::HelperOp::Heartbeat,
                                       json(heartbeat_req));
        ok = expect(heartbeat.success,
                    "Heartbeat should bind registry cleanup before unlocked test") &&
             ok;

        std::atomic<bool> probe_returned{false};
        bool hook_saw_unlocked_state = false;
        std::thread probe_thread;
        exv::core::lifecycle::testing::set_compare_delete_quarantine_hook(
            [&](const std::string&, const std::string&) {
                probe_thread = std::thread([&] {
                    (void)handler.has_active_core_lease();
                    probe_returned.store(true);
                });
                std::this_thread::sleep_for(std::chrono::milliseconds(100));
                hook_saw_unlocked_state = probe_returned.load();
            });

        exv::helper::CleanupRequest cleanup_req;
        cleanup_req.session_id = start_resp.session_id;
        auto cleanup = dispatch_json(handler, exv::helper::HelperOp::Cleanup,
                                     json(cleanup_req));
        exv::core::lifecycle::testing::set_compare_delete_quarantine_hook(
            nullptr);
        if (probe_thread.joinable()) {
            probe_thread.join();
        }

        ok = expect(cleanup.success,
                    "unlocked cleanup test should cleanup successfully") && ok;
        ok = expect(hook_saw_unlocked_state,
                    "registry compare-delete must not run while helper state_mutex_ is held") &&
             ok;

        fs::remove_all(root, ec);
    }

    // --- finalize handoff removes records without deleting the core registry ---
    {
        namespace fs = std::filesystem;
        const auto root = fs::temp_directory_path() /
            ("ecnuvpn-helper-core-registry-finalize-" +
             std::to_string(current_process_id()));
        std::error_code ec;
        fs::remove_all(root, ec);
        fs::create_directories(root, ec);

        ecnuvpn::runtime::bootstrap(root.string(), root.string(), true);

        exv::helper::HelperHandler handler;
        exv::helper::HelperStartupContext startup;
        startup.launch_mode = "oneshot";
        handler.set_startup_context(startup);

        exv::helper::AcquireCoreLeaseRequest acquire_req;
        acquire_req.core_pid = 7171;
        acquire_req.purpose = "handoff";
        auto acquire = dispatch_json(
            handler, exv::helper::HelperOp::AcquireCoreLease, json(acquire_req));
        ok = expect(acquire.success,
                    "AcquireCoreLease should succeed before finalize handoff test") &&
             ok;
        const auto acquire_resp =
            exv::helper::acquire_core_lease_response_from_json(
                json::parse(acquire.payload_json));

        exv::helper::StartSessionRequest start_req;
        start_req.profile_id.value = "profile-helper-finalize";
        auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                                   json(start_req));
        ok = expect(start.success,
                    "StartSession should succeed before finalize handoff test") &&
             ok;
        const auto start_resp = exv::helper::start_session_response_from_json(
            json::parse(start.payload_json));

        const auto registry_path =
            exv::core::lifecycle::core_registry_path(root.string());
        const auto snapshot =
            make_core_registry_snapshot(root, acquire_req.core_pid,
                                        acquire_resp.lease_id);
        ok = expect(exv::core::lifecycle::write_core_registry(snapshot,
                                                              registry_path),
                    "finalize handoff test should write versioned registry") &&
             ok;

        exv::helper::HeartbeatRequest heartbeat_req;
        heartbeat_req.session_id = start_resp.session_id;
        heartbeat_req.core_phase = "handoff";
        auto heartbeat = dispatch_json(handler, exv::helper::HelperOp::Heartbeat,
                                       json(heartbeat_req));
        ok = expect(heartbeat.success,
                    "Heartbeat should bind registry cleanup before finalize") && ok;

        exv::helper::FinalizeHandoffRequest finalize_req;
        finalize_req.exit = true;
        auto finalize = dispatch_json(
            handler, exv::helper::HelperOp::FinalizeHandoff, json(finalize_req));
        ok = expect(finalize.success,
                    "FinalizeHandoff should succeed in oneshot mode") && ok;
        ok = expect(fs::exists(registry_path),
                    "FinalizeHandoff must preserve the versioned core registry") &&
             ok;

        fs::remove_all(root, ec);
    }

    // --- partial production cleanup must keep the versioned core registry ---
    {
        namespace fs = std::filesystem;
        const auto root = fs::temp_directory_path() /
            ("ecnuvpn-helper-core-registry-partial-" +
             std::to_string(current_process_id()));
        std::error_code ec;
        fs::remove_all(root, ec);
        fs::create_directories(root, ec);

        ecnuvpn::runtime::bootstrap(root.string(), root.string(), true);

        exv::helper::HelperHandler handler;

        exv::helper::AcquireCoreLeaseRequest acquire_req;
        acquire_req.core_pid = 6160;
        acquire_req.purpose = "connect";
        auto acquire = dispatch_json(
            handler, exv::helper::HelperOp::AcquireCoreLease, json(acquire_req));
        ok = expect(acquire.success,
                    "AcquireCoreLease should succeed before partial cleanup test") &&
             ok;
        const auto acquire_resp =
            exv::helper::acquire_core_lease_response_from_json(
                json::parse(acquire.payload_json));

        exv::helper::StartSessionRequest start_req;
        start_req.profile_id.value = "profile-helper-partial";
        auto start = dispatch_json(handler, exv::helper::HelperOp::StartSession,
                                   json(start_req));
        ok = expect(start.success,
                    "StartSession should succeed before partial cleanup test") && ok;
        const auto start_resp = exv::helper::start_session_response_from_json(
            json::parse(start.payload_json));

        const auto registry_path = exv::core::lifecycle::core_registry_path(root.string());
        const auto snapshot =
            make_core_registry_snapshot(root, acquire_req.core_pid,
                                        acquire_resp.lease_id);
        ok = expect(exv::core::lifecycle::write_core_registry(snapshot, registry_path),
                    "partial cleanup test should write versioned registry") && ok;

        exv::helper::HeartbeatRequest heartbeat_req;
        heartbeat_req.session_id = start_resp.session_id;
        heartbeat_req.core_phase = "connected";
        auto heartbeat = dispatch_json(handler, exv::helper::HelperOp::Heartbeat,
                                       json(heartbeat_req));
        ok = expect(heartbeat.success,
                    "Heartbeat should succeed before partial cleanup test") && ok;

        handler.cleanup_registry().add_resource(start_resp.session_id,
                                                {"route", "0.0.0.0/0"});

        exv::helper::CleanupRequest cleanup_req;
        cleanup_req.session_id = start_resp.session_id;
        auto cleanup = dispatch_json(handler, exv::helper::HelperOp::Cleanup,
                                     json(cleanup_req));
        ok = expect(!cleanup.success,
                    "partial cleanup should fail when managed resources cannot be removed") &&
             ok;
        ok = expect(fs::exists(registry_path),
                    "partial cleanup must keep the versioned core registry") && ok;

        fs::remove_all(root, ec);
    }

    if (ok) {
        std::cout << "helper_cleanup_registry_test: all assertions passed\n";
    } else {
        std::cerr << "helper_cleanup_registry_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
