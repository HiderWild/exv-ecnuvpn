#include "core/lifecycle/core_registry.hpp"

#include "core/lifecycle/core_paths.hpp"

#include <atomic>
#include <chrono>
#include <ctime>
#include <filesystem>
#include <functional>
#include <fstream>
#include <iomanip>
#include <mutex>
#include <sstream>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif

namespace exv::core::lifecycle {
namespace testing {
using CoreRegistryCompareDeleteHook =
    std::function<void(const std::string& final_path,
                       const std::string& tombstone_path)>;
} // namespace testing

namespace {

int current_process_id() {
#ifdef _WIN32
    return static_cast<int>(GetCurrentProcessId());
#else
    return static_cast<int>(getpid());
#endif
}

std::uint64_t next_temp_suffix() {
    static std::atomic<std::uint64_t> next_suffix{0};
    return next_suffix.fetch_add(1, std::memory_order_relaxed);
}

std::mutex& registry_file_mutex() {
    static std::mutex mutex;
    return mutex;
}

std::filesystem::path unique_temp_path(const std::filesystem::path& final_path) {
    return final_path.parent_path() /
        (final_path.filename().string() + ".tmp." +
         std::to_string(current_process_id()) + "." +
         std::to_string(next_temp_suffix()));
}

std::filesystem::path
unique_tombstone_path(const std::filesystem::path& final_path) {
    const auto tick = std::chrono::steady_clock::now()
                          .time_since_epoch()
                          .count();
    return final_path.parent_path() /
        (final_path.filename().string() + ".delete." +
         std::to_string(current_process_id()) + "." +
         std::to_string(tick) + "." + std::to_string(next_temp_suffix()));
}

testing::CoreRegistryCompareDeleteHook& compare_delete_quarantine_hook() {
    static testing::CoreRegistryCompareDeleteHook hook;
    return hook;
}

bool replace_file_atomically(const std::filesystem::path& source_path,
                             const std::filesystem::path& final_path) {
#ifdef _WIN32
    return MoveFileExA(source_path.string().c_str(), final_path.string().c_str(),
                       MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) != 0;
#else
    std::error_code ec;
    std::filesystem::rename(source_path, final_path, ec);
    return !ec;
#endif
}

std::string iso8601_now() {
    const auto now = std::chrono::system_clock::now();
    const auto seconds = std::chrono::time_point_cast<std::chrono::seconds>(now);
    const auto time = std::chrono::system_clock::to_time_t(seconds);
    std::tm utc = {};
#ifdef _WIN32
    gmtime_s(&utc, &time);
#else
    gmtime_r(&time, &utc);
#endif

    const auto millis =
        std::chrono::duration_cast<std::chrono::milliseconds>(now - seconds);

    std::ostringstream out;
    out << std::put_time(&utc, "%Y-%m-%dT%H:%M:%S") << '.'
        << std::setw(3) << std::setfill('0') << millis.count() << 'Z';
    return out.str();
}

nlohmann::json to_json(const CoreRegistrySnapshot& snapshot) {
    return nlohmann::json{
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
}

std::optional<CoreRegistrySnapshot> snapshot_from_json(const nlohmann::json& json) {
    try {
        CoreRegistrySnapshot snapshot;
        snapshot.core_instance_id = json.at("core_instance_id").get<std::string>();
        snapshot.pid = json.at("pid").get<int>();
        snapshot.core_path = json.at("core_path").get<std::string>();
        snapshot.ipc_path = json.at("ipc_path").get<std::string>();
        snapshot.ipc_protocol_version =
            json.at("ipc_protocol_version").get<std::string>();
        snapshot.app_version = json.at("app_version").get<std::string>();
        snapshot.contract_version = json.at("contract_version").get<std::string>();
        snapshot.started_at = json.at("started_at").get<std::string>();
        snapshot.last_heartbeat_at =
            json.at("last_heartbeat_at").get<std::string>();
        snapshot.last_known_tunnel_phase =
            json.at("last_known_tunnel_phase").get<std::string>();
        snapshot.last_known_connected =
            json.at("last_known_connected").get<bool>();
        snapshot.last_known_network_ready =
            json.at("last_known_network_ready").get<bool>();
        snapshot.helper_core_lease_id =
            json.at("helper_core_lease_id").get<std::string>();
        return snapshot;
    } catch (...) {
        return std::nullopt;
    }
}

bool write_json_atomic(const std::filesystem::path& final_path,
                       const nlohmann::json& json) {
    std::lock_guard<std::mutex> lock(registry_file_mutex());

    std::error_code ec;
    const auto parent_path = final_path.parent_path();
    if (!parent_path.empty()) {
        std::filesystem::create_directories(parent_path, ec);
        if (ec) {
            return false;
        }
    }

    const auto tmp_path = unique_temp_path(final_path);

    {
        std::ofstream out(tmp_path, std::ios::out | std::ios::trunc);
        if (!out.is_open()) {
            return false;
        }
        out << json.dump(2);
        out.flush();
        if (!out.good()) {
            std::filesystem::remove(tmp_path, ec);
            return false;
        }
    }

    if (replace_file_atomically(tmp_path, final_path)) {
        return true;
    }

    std::filesystem::remove(tmp_path, ec);
    return false;
}

bool snapshot_matches_delete_expected(const CoreRegistrySnapshot& snapshot,
                                      const CoreRegistryDeleteMatch& expected) {
    return snapshot.core_instance_id == expected.core_instance_id &&
           snapshot.pid == expected.pid &&
           snapshot.helper_core_lease_id == expected.helper_core_lease_id &&
           snapshot.ipc_protocol_version == expected.ipc_protocol_version;
}

void preserve_quarantined_registry(
    const std::filesystem::path& final_path,
    const std::filesystem::path& tombstone_path) {
    namespace fs = std::filesystem;

    std::error_code ec;
    const bool final_exists = fs::exists(final_path, ec);
    if (!ec && !final_exists) {
        fs::rename(tombstone_path, final_path, ec);
        return;
    }
    if (!ec && final_exists) {
        fs::remove(tombstone_path, ec);
    }
}

} // namespace

bool write_core_registry(const CoreRegistrySnapshot& snapshot) {
    return write_core_registry(snapshot, core_registry_path());
}

bool write_core_registry(const CoreRegistrySnapshot& snapshot,
                         const std::string& registry_path) {
    return write_json_atomic(registry_path, to_json(snapshot));
}

CoreRegistryReadResult read_core_registry() {
    return read_core_registry(core_registry_path());
}

CoreRegistryReadResult read_core_registry(const std::string& registry_path) {
    namespace fs = std::filesystem;

    CoreRegistryReadResult result;
    std::error_code ec;
    if (!fs::exists(registry_path, ec)) {
        result.state = CoreRegistryReadState::missing;
        return result;
    }
    if (ec || !fs::is_regular_file(registry_path, ec)) {
        result.state = CoreRegistryReadState::unknown_state;
        return result;
    }

    try {
        std::ifstream in(registry_path);
        if (!in.is_open()) {
            result.state = CoreRegistryReadState::unknown_state;
            return result;
        }
        const auto parsed = nlohmann::json::parse(in);
        auto snapshot = snapshot_from_json(parsed);
        if (!snapshot.has_value()) {
            result.state = CoreRegistryReadState::unknown_state;
            return result;
        }
        result.state = CoreRegistryReadState::present;
        result.snapshot = std::move(snapshot);
        return result;
    } catch (...) {
        result.state = CoreRegistryReadState::unknown_state;
        return result;
    }
}

CoreRegistryDeleteMatch core_registry_delete_match(
    const CoreRegistrySnapshot& snapshot) {
    CoreRegistryDeleteMatch expected;
    expected.core_instance_id = snapshot.core_instance_id;
    expected.pid = snapshot.pid;
    expected.helper_core_lease_id = snapshot.helper_core_lease_id;
    expected.ipc_protocol_version = snapshot.ipc_protocol_version;
    return expected;
}

bool compare_and_delete_core_registry(const CoreRegistryDeleteMatch& expected) {
    return compare_and_delete_core_registry(core_registry_path(), expected);
}

bool compare_and_delete_core_registry(const std::string& registry_path,
                                      const CoreRegistryDeleteMatch& expected) {
    namespace fs = std::filesystem;

    const fs::path final_path(registry_path);
    std::lock_guard<std::mutex> lock(registry_file_mutex());

    std::error_code ec;
    if (!fs::exists(final_path, ec) || ec) {
        return false;
    }
    if (!fs::is_regular_file(final_path, ec) || ec) {
        return false;
    }

    const fs::path tombstone_path = unique_tombstone_path(final_path);
    fs::rename(final_path, tombstone_path, ec);
    if (ec) {
        return false;
    }

    auto& hook = compare_delete_quarantine_hook();
    if (hook) {
        hook(final_path.string(), tombstone_path.string());
    }

    const auto loaded = read_core_registry(tombstone_path.string());
    if (loaded.state != CoreRegistryReadState::present ||
        !loaded.snapshot.has_value()) {
        preserve_quarantined_registry(final_path, tombstone_path);
        return false;
    }

    const auto& snapshot = *loaded.snapshot;
    if (!snapshot_matches_delete_expected(snapshot, expected)) {
        preserve_quarantined_registry(final_path, tombstone_path);
        return false;
    }

    const bool removed = fs::remove(tombstone_path, ec);
    return removed && !ec;
}

namespace testing {

void set_compare_delete_quarantine_hook(
    CoreRegistryCompareDeleteHook hook) {
    std::lock_guard<std::mutex> lock(registry_file_mutex());
    compare_delete_quarantine_hook() = std::move(hook);
}

} // namespace testing

void refresh_core_registry_heartbeat(CoreRegistrySnapshot& snapshot) {
    snapshot.last_heartbeat_at = iso8601_now();
}

void apply_tunnel_status(CoreRegistrySnapshot& snapshot,
                         const exv::core::TunnelStatusSnapshot& status) {
    snapshot.last_known_tunnel_phase = tunnel_phase_wire_name(status.phase);
    snapshot.last_known_connected = status.phase == exv::core::TunnelPhase::Connected;
    snapshot.last_known_network_ready = status.network_ready;
}

CoreRegistrySnapshot core_registry_snapshot_from_hello(
    const nlohmann::json& hello_payload, const std::string& ipc_path) {
    CoreRegistrySnapshot snapshot;
    snapshot.core_instance_id =
        hello_payload.value("core_instance_id", std::string());
    snapshot.pid = hello_payload.value("pid", 0);
    snapshot.core_path = hello_payload.value("core_path", std::string());
    snapshot.ipc_path = ipc_path;
    snapshot.ipc_protocol_version =
        hello_payload.value("ipc_protocol_version", ipc_protocol_name());
    snapshot.app_version = hello_payload.value("app_version", std::string());
    snapshot.contract_version =
        hello_payload.value("contract_version", std::string());
    snapshot.started_at = hello_payload.value("started_at", iso8601_now());
    snapshot.last_heartbeat_at = snapshot.started_at;
    snapshot.last_known_tunnel_phase = "idle";
    snapshot.last_known_connected = false;
    snapshot.last_known_network_ready = false;
    snapshot.helper_core_lease_id =
        hello_payload.value("helper_core_lease_id", std::string());
    return snapshot;
}

} // namespace exv::core::lifecycle
