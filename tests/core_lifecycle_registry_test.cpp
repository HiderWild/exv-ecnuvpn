#include "core/lifecycle/core_lock.hpp"
#include "core/lifecycle/core_paths.hpp"
#include "core/lifecycle/core_registry.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <optional>
#include <string>
#include <vector>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif

namespace {

bool expect(bool condition, const char* message) {
    if (condition) {
        return true;
    }
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

std::filesystem::path make_temp_dir(const std::string& tag) {
    namespace fs = std::filesystem;
    const auto root = fs::temp_directory_path() /
        (tag + "-" + std::to_string(current_process_id()));
    std::error_code ec;
    fs::remove_all(root, ec);
    fs::create_directories(root, ec);
    return root;
}

exv::core::lifecycle::CoreRegistrySnapshot make_snapshot(
    const std::string& state_dir) {
    exv::core::lifecycle::CoreRegistrySnapshot snapshot;
    snapshot.core_instance_id = "core-instance-1";
    snapshot.pid = 4242;
    snapshot.core_path = "C:/Program Files/ECNU-VPN/exv.exe";
    snapshot.ipc_path = exv::core::lifecycle::core_ipc_path(state_dir);
    snapshot.ipc_protocol_version = exv::core::lifecycle::ipc_protocol_name();
    snapshot.app_version = "3.3.0";
    snapshot.contract_version = "2026-06-16.cli-core-ui-contract.v1";
    snapshot.started_at = "2026-06-16T12:00:00.000Z";
    snapshot.last_heartbeat_at = "2026-06-16T12:00:01.000Z";
    snapshot.last_known_tunnel_phase = "idle";
    snapshot.last_known_connected = false;
    snapshot.last_known_network_ready = false;
    snapshot.helper_core_lease_id = "core-lease-1";
    return snapshot;
}

} // namespace

int main() {
    namespace fs = std::filesystem;
    using exv::core::lifecycle::CoreInstanceLock;
    using exv::core::lifecycle::CoreRegistryReadState;

    bool ok = true;

    {
        const std::string state_dir = make_temp_dir("ecnuvpn-core-paths-test").string();
        const auto ipc_path = exv::core::lifecycle::core_ipc_path(state_dir);
        const auto lock_path = exv::core::lifecycle::core_lock_path(state_dir);
        const auto registry_path =
            exv::core::lifecycle::core_registry_path(state_dir);

        ok = expect(exv::core::lifecycle::ipc_protocol_name() == "ipc-v1",
                    "ipc protocol name should be ipc-v1") && ok;
        ok = expect(ipc_path.find("ipc-v1") != std::string::npos,
                    "core_ipc_path should contain ipc-v1") && ok;
        ok = expect(lock_path.find("ipc-v1") != std::string::npos,
                    "core_lock_path should contain ipc-v1") && ok;
        ok = expect(registry_path.find("ipc-v1") != std::string::npos,
                    "core_registry_path should contain ipc-v1") && ok;
    }

    {
        const std::string state_dir = make_temp_dir("ecnuvpn-core-lock-test").string();
        auto first = CoreInstanceLock::try_acquire(state_dir);
        ok = expect(first.has_value(),
                    "first lock acquisition should succeed") && ok;

        auto second = CoreInstanceLock::try_acquire(state_dir);
        ok = expect(!second.has_value(),
                    "second lock acquisition should fail while first is alive") && ok;

        first.reset();

        auto third = CoreInstanceLock::try_acquire(state_dir);
        ok = expect(third.has_value(),
                    "lock should be released after first owner is destroyed") && ok;
    }

    {
        const fs::path root = make_temp_dir("ecnuvpn-core-registry-test");
        const std::string state_dir = root.string();
        const auto registry_path = exv::core::lifecycle::core_registry_path(state_dir);
        const auto snapshot = make_snapshot(state_dir);

        ok = expect(exv::core::lifecycle::write_core_registry(snapshot, registry_path),
                    "write_core_registry should succeed") && ok;
        ok = expect(fs::exists(registry_path),
                    "registry file should exist after write") && ok;

        std::vector<fs::path> leftovers;
        for (const auto& entry : fs::directory_iterator(root)) {
            const auto name = entry.path().filename().string();
            if (name.find(".tmp.") != std::string::npos) {
                leftovers.push_back(entry.path());
            }
        }
        ok = expect(leftovers.empty(),
                    "atomic registry write should not leave temp files behind") && ok;

        const auto loaded = exv::core::lifecycle::read_core_registry(registry_path);
        ok = expect(loaded.state == CoreRegistryReadState::present,
                    "written registry should be readable") && ok;
        ok = expect(loaded.snapshot.has_value(),
                    "readable registry should return a snapshot") && ok;
        if (loaded.snapshot.has_value()) {
            ok = expect(loaded.snapshot->core_instance_id == snapshot.core_instance_id,
                        "loaded registry should preserve core_instance_id") && ok;
            ok = expect(loaded.snapshot->pid == snapshot.pid,
                        "loaded registry should preserve pid") && ok;
            ok = expect(loaded.snapshot->ipc_path == snapshot.ipc_path,
                        "loaded registry should preserve ipc_path") && ok;
            ok = expect(
                loaded.snapshot->helper_core_lease_id == snapshot.helper_core_lease_id,
                "loaded registry should preserve helper_core_lease_id") && ok;
        }
    }

    {
        const fs::path root = make_temp_dir("ecnuvpn-core-registry-missing");
        const auto registry_path =
            exv::core::lifecycle::core_registry_path(root.string());
        const auto missing = exv::core::lifecycle::read_core_registry(registry_path);
        ok = expect(missing.state == CoreRegistryReadState::missing,
                    "missing registry should report missing state") && ok;
        ok = expect(!missing.snapshot.has_value(),
                    "missing registry should not return a snapshot") && ok;
    }

    {
        const fs::path root = make_temp_dir("ecnuvpn-core-registry-corrupt");
        const auto registry_path =
            exv::core::lifecycle::core_registry_path(root.string());
        std::ofstream out(registry_path, std::ios::out | std::ios::trunc);
        out << "{ definitely-not-json";
        out.close();

        const auto loaded = exv::core::lifecycle::read_core_registry(registry_path);
        ok = expect(loaded.state == CoreRegistryReadState::unknown_state,
                    "corrupt registry should report unknown_state") && ok;
        ok = expect(!loaded.snapshot.has_value(),
                    "corrupt registry should not return a snapshot") && ok;
    }

    {
        const fs::path root = make_temp_dir("ecnuvpn-core-registry-delete");
        const std::string state_dir = root.string();
        const auto registry_path =
            exv::core::lifecycle::core_registry_path(state_dir);
        const auto snapshot = make_snapshot(state_dir);

        ok = expect(exv::core::lifecycle::write_core_registry(snapshot, registry_path),
                    "compare/delete test should write registry first") && ok;

        auto match = exv::core::lifecycle::core_registry_delete_match(snapshot);

        auto mismatch_instance = match;
        mismatch_instance.core_instance_id = "other-instance";
        ok = expect(!exv::core::lifecycle::compare_and_delete_core_registry(
                        registry_path, mismatch_instance),
                    "compare/delete should reject mismatched core_instance_id") && ok;
        ok = expect(fs::exists(registry_path),
                    "registry must remain after core_instance_id mismatch") && ok;

        auto mismatch_pid = match;
        mismatch_pid.pid += 1;
        ok = expect(!exv::core::lifecycle::compare_and_delete_core_registry(
                        registry_path, mismatch_pid),
                    "compare/delete should reject mismatched pid") && ok;
        ok = expect(fs::exists(registry_path),
                    "registry must remain after pid mismatch") && ok;

        auto mismatch_lease = match;
        mismatch_lease.helper_core_lease_id = "different-lease";
        ok = expect(!exv::core::lifecycle::compare_and_delete_core_registry(
                        registry_path, mismatch_lease),
                    "compare/delete should reject mismatched helper_core_lease_id") &&
             ok;
        ok = expect(fs::exists(registry_path),
                    "registry must remain after helper_core_lease_id mismatch") && ok;

        auto mismatch_protocol = match;
        mismatch_protocol.ipc_protocol_version = "ipc-v2";
        ok = expect(!exv::core::lifecycle::compare_and_delete_core_registry(
                        registry_path, mismatch_protocol),
                    "compare/delete should reject mismatched ipc protocol version") &&
             ok;
        ok = expect(fs::exists(registry_path),
                    "registry must remain after protocol mismatch") && ok;

        ok = expect(exv::core::lifecycle::compare_and_delete_core_registry(
                        registry_path, match),
                    "compare/delete should delete matching registry") && ok;
        ok = expect(!fs::exists(registry_path),
                    "matching compare/delete should remove registry file") && ok;
    }

    if (ok) {
        std::cout << "core_lifecycle_registry_test: all assertions passed\n";
        return 0;
    }

    std::cerr << "core_lifecycle_registry_test: some assertions FAILED\n";
    return 1;
}
