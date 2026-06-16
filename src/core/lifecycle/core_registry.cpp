#include "core/lifecycle/core_registry.hpp"

#include "core/lifecycle/core_paths.hpp"

#include <chrono>
#include <ctime>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <sstream>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif

namespace exv::core::lifecycle {
namespace {

int current_process_id() {
#ifdef _WIN32
    return static_cast<int>(GetCurrentProcessId());
#else
    return static_cast<int>(getpid());
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
    std::error_code ec;
    std::filesystem::create_directories(final_path.parent_path(), ec);
    if (ec) {
        return false;
    }

    const auto tmp_path =
        final_path.string() + ".tmp." + std::to_string(current_process_id());

    {
        std::ofstream out(tmp_path, std::ios::out | std::ios::trunc);
        if (!out.is_open()) {
            return false;
        }
        out << json.dump(2);
        if (!out.good()) {
            return false;
        }
    }

    std::filesystem::remove(final_path, ec);
    ec.clear();
    std::filesystem::rename(tmp_path, final_path, ec);
    if (!ec) {
        return true;
    }

    std::error_code copy_ec;
    std::filesystem::copy_file(
        tmp_path, final_path, std::filesystem::copy_options::overwrite_existing,
        copy_ec);
    std::filesystem::remove(tmp_path, ec);
    return !copy_ec;
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
    const auto loaded = read_core_registry(registry_path);
    if (loaded.state != CoreRegistryReadState::present ||
        !loaded.snapshot.has_value()) {
        return false;
    }

    const auto& snapshot = *loaded.snapshot;
    if (snapshot.core_instance_id != expected.core_instance_id ||
        snapshot.pid != expected.pid ||
        snapshot.helper_core_lease_id != expected.helper_core_lease_id ||
        snapshot.ipc_protocol_version != expected.ipc_protocol_version) {
        return false;
    }

    std::error_code ec;
    const bool removed = std::filesystem::remove(registry_path, ec);
    return removed && !ec;
}

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
    snapshot.helper_core_lease_id.clear();
    return snapshot;
}

} // namespace exv::core::lifecycle
