#pragma once

#include <nlohmann/json.hpp>

#include <optional>
#include <string>
#include <string_view>

namespace exv::core::lifecycle {

std::string ipc_protocol_name();

std::string core_ipc_path();
std::string core_ipc_path(const std::string& state_dir);

std::string core_lock_path();
std::string core_lock_path(const std::string& state_dir);

std::string core_registry_path();
std::string core_registry_path(const std::string& state_dir);

bool accepts_contract_version(std::string_view requested);

struct CoreRegistrySnapshot {
    std::string core_instance_id;
    int pid = 0;
    std::string core_path;
    std::string ipc_path;
    std::string ipc_protocol_version;
    std::string app_version;
    std::string contract_version;
    std::string started_at;
    std::string last_heartbeat_at;
    std::string last_known_tunnel_phase = "idle";
    bool last_known_connected = false;
    bool last_known_network_ready = false;
    std::string helper_core_lease_id;
};

enum class CoreRegistryReadState {
    present,
    missing,
    unknown_state,
};

struct CoreRegistryReadResult {
    CoreRegistryReadState state = CoreRegistryReadState::missing;
    std::optional<CoreRegistrySnapshot> snapshot;
};

CoreRegistryReadResult read_core_registry();
CoreRegistryReadResult read_core_registry(const std::string& registry_path);

nlohmann::json core_registry_to_json(const CoreRegistrySnapshot& snapshot);

} // namespace exv::core::lifecycle
