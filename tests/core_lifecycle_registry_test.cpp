#include "core/lifecycle/core_lock.hpp"
#include "core/lifecycle/core_paths.hpp"
#include "core/lifecycle/core_registry.hpp"

#include <filesystem>
#include <fstream>
#include <functional>
#include <iostream>
#include <atomic>
#include <optional>
#include <sstream>
#include <string>
#include <system_error>
#include <thread>
#include <vector>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif

#ifndef ECNUVPN_SOURCE_DIR
#define ECNUVPN_SOURCE_DIR "."
#endif

namespace exv::core::lifecycle::testing {
using CoreRegistryCompareDeleteHook =
    std::function<void(const std::string& final_path,
                       const std::string& tombstone_path)>;

void set_compare_delete_quarantine_hook(
    CoreRegistryCompareDeleteHook hook);

using CoreRegistryExistsHook =
    std::function<bool(const std::filesystem::path& path,
                       std::error_code& ec)>;

void set_read_core_registry_exists_hook(CoreRegistryExistsHook hook);
} // namespace exv::core::lifecycle::testing

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

std::string read_file(const std::filesystem::path& path) {
    std::ifstream input(path);
    std::ostringstream contents;
    contents << input.rdbuf();
    return contents.str();
}

bool write_snapshot_directly(
    const exv::core::lifecycle::CoreRegistrySnapshot& snapshot,
    const std::filesystem::path& path) {
    namespace fs = std::filesystem;
    std::error_code ec;
    fs::create_directories(path.parent_path(), ec);
    if (ec) {
        return false;
    }

    nlohmann::json payload{
        {"core_instance_id", snapshot.core_instance_id},
        {"pid", snapshot.pid},
        {"core_path", snapshot.core_path},
        {"ipc_path", snapshot.ipc_path},
        {"ipc_protocol_version", snapshot.ipc_protocol_version},
        {"app_version", snapshot.app_version},
        {"contract_version", snapshot.contract_version},
        {"started_at", snapshot.started_at},
        {"last_heartbeat_at", snapshot.last_heartbeat_at},
        {"last_known_tunnel_phase", snapshot.last_known_tunnel_phase},
        {"last_known_connected", snapshot.last_known_connected},
        {"last_known_network_ready", snapshot.last_known_network_ready},
        {"helper_core_lease_id", snapshot.helper_core_lease_id},
    };

    std::ofstream out(path, std::ios::out | std::ios::trunc);
    if (!out.is_open()) {
        return false;
    }
    out << payload.dump(2);
    return out.good();
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
        const fs::path root = make_temp_dir("ecnuvpn-core-registry-concurrent");
        const std::string state_dir = root.string();
        const auto registry_path =
            exv::core::lifecycle::core_registry_path(state_dir);

        std::atomic<bool> writes_ok{true};
        std::vector<std::thread> writers;
        for (int writer = 0; writer < 8; ++writer) {
            writers.emplace_back([&, writer] {
                for (int iteration = 0; iteration < 20; ++iteration) {
                    auto snapshot = make_snapshot(state_dir);
                    snapshot.core_instance_id =
                        "core-instance-" + std::to_string(writer) + "-" +
                        std::to_string(iteration);
                    snapshot.pid = 5000 + writer;
                    snapshot.helper_core_lease_id =
                        "core-lease-" + std::to_string(iteration);
                    snapshot.last_known_connected = (iteration % 2) == 0;
                    if (!exv::core::lifecycle::write_core_registry(
                            snapshot, registry_path)) {
                        writes_ok.store(false);
                    }
                }
            });
        }
        for (auto& writer : writers) {
            writer.join();
        }

        ok = expect(writes_ok.load(),
                    "concurrent registry writes should all succeed") && ok;

        const auto loaded = exv::core::lifecycle::read_core_registry(registry_path);
        ok = expect(loaded.state == CoreRegistryReadState::present,
                    "concurrent registry writes should leave a readable registry") &&
             ok;
        ok = expect(loaded.snapshot.has_value(),
                    "concurrent registry writes should keep a final snapshot") && ok;

        std::vector<fs::path> leftovers;
        for (const auto& entry : fs::directory_iterator(root)) {
            const auto name = entry.path().filename().string();
            if (name.find(".tmp.") != std::string::npos) {
                leftovers.push_back(entry.path());
            }
        }
        ok = expect(leftovers.empty(),
                    "concurrent registry writes should not leave temp files behind") &&
             ok;
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
        exv::core::lifecycle::testing::set_read_core_registry_exists_hook(
            [](const std::filesystem::path&, std::error_code& ec) {
                ec = std::make_error_code(std::errc::permission_denied);
                return false;
            });
        const auto loaded =
            exv::core::lifecycle::read_core_registry("error-is-not-missing");
        exv::core::lifecycle::testing::set_read_core_registry_exists_hook(
            nullptr);

        ok = expect(loaded.state == CoreRegistryReadState::unknown_state,
                    "filesystem errors while probing registry must not be reported as missing") &&
             ok;
        ok = expect(!loaded.snapshot.has_value(),
                    "filesystem probe errors should not return a snapshot") && ok;
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

    {
        const fs::path root =
            make_temp_dir("ecnuvpn-core-registry-delete-quarantine");
        const std::string state_dir = root.string();
        const auto registry_path =
            exv::core::lifecycle::core_registry_path(state_dir);
        const auto old_snapshot = make_snapshot(state_dir);
        auto replacement_snapshot = make_snapshot(state_dir);
        replacement_snapshot.core_instance_id = "core-instance-replacement";
        replacement_snapshot.pid = 9001;
        replacement_snapshot.helper_core_lease_id = "core-lease-replacement";

        ok = expect(exv::core::lifecycle::write_core_registry(old_snapshot,
                                                              registry_path),
                    "quarantine delete test should write old registry") && ok;

        bool hook_called = false;
        bool hook_ok = true;
        exv::core::lifecycle::testing::set_compare_delete_quarantine_hook(
            [&](const std::string& final_path,
                const std::string& tombstone_path) {
                hook_called = true;
                hook_ok =
                    expect(!fs::exists(final_path),
                           "quarantine hook should observe final path moved away") &&
                    hook_ok;
                hook_ok =
                    expect(fs::exists(tombstone_path),
                           "quarantine hook should observe tombstone file") &&
                    hook_ok;
                hook_ok =
                    expect(write_snapshot_directly(replacement_snapshot,
                                                   final_path),
                           "quarantine hook should recreate final registry") &&
                    hook_ok;
            });

        const bool deleted =
            exv::core::lifecycle::compare_and_delete_core_registry(
                registry_path,
                exv::core::lifecycle::core_registry_delete_match(old_snapshot));
        exv::core::lifecycle::testing::set_compare_delete_quarantine_hook(
            nullptr);

        ok = expect(hook_called,
                    "compare/delete should quarantine before comparing") &&
             ok;
        ok = hook_ok && ok;
        ok = expect(deleted,
                    "compare/delete should delete the quarantined matching old registry") &&
             ok;

        const auto loaded =
            exv::core::lifecycle::read_core_registry(registry_path);
        ok = expect(loaded.state == CoreRegistryReadState::present,
                    "replacement registry should remain after compare/delete") &&
             ok;
        ok = expect(loaded.snapshot.has_value(),
                    "replacement registry should still be readable") &&
             ok;
        if (loaded.snapshot.has_value()) {
            ok = expect(loaded.snapshot->core_instance_id ==
                            replacement_snapshot.core_instance_id,
                        "compare/delete must not delete replacement core_instance_id") &&
                 ok;
            ok = expect(loaded.snapshot->pid == replacement_snapshot.pid,
                        "compare/delete must not delete replacement pid") &&
                 ok;
            ok = expect(loaded.snapshot->helper_core_lease_id ==
                            replacement_snapshot.helper_core_lease_id,
                        "compare/delete must not delete replacement lease") &&
                 ok;
        }
    }

    {
        const auto source_path = fs::path(ECNUVPN_SOURCE_DIR) / "src" / "core" /
            "lifecycle" / "core_registry.cpp";
        const auto source = read_file(source_path);
        ok = expect(!source.empty(),
                    "core_registry.cpp source should be readable for contract checks") &&
             ok;
        ok = expect(source.find("copy_file(") == std::string::npos,
                    "core registry writes must not fall back to copy_file") && ok;
        ok = expect(source.find("remove(final_path") == std::string::npos,
                    "core registry writes must not delete the final path before replace") &&
             ok;
        ok = expect(source.find("std::filesystem::remove(registry_path") ==
                        std::string::npos,
                    "core registry compare/delete must not remove the final path directly") &&
             ok;
    }

    if (ok) {
        std::cout << "core_lifecycle_registry_test: all assertions passed\n";
        return 0;
    }

    std::cerr << "core_lifecycle_registry_test: some assertions FAILED\n";
    return 1;
}
