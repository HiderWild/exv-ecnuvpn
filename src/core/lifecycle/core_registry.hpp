#pragma once

#include "core/tunnel_controller/tunnel_state.hpp"

#include <nlohmann/json.hpp>

#include <optional>
#include <string>

namespace exv::core::lifecycle {

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

struct CoreRegistryDeleteMatch {
    std::string core_instance_id;
    int pid = 0;
    std::string helper_core_lease_id;
    std::string ipc_protocol_version;
};

bool write_core_registry(const CoreRegistrySnapshot& snapshot);
bool write_core_registry(const CoreRegistrySnapshot& snapshot,
                         const std::string& registry_path);

CoreRegistryReadResult read_core_registry();
CoreRegistryReadResult read_core_registry(const std::string& registry_path);

CoreRegistryDeleteMatch core_registry_delete_match(
    const CoreRegistrySnapshot& snapshot);

bool compare_and_delete_core_registry(const CoreRegistryDeleteMatch& expected);
bool compare_and_delete_core_registry(const std::string& registry_path,
                                      const CoreRegistryDeleteMatch& expected);

void refresh_core_registry_heartbeat(CoreRegistrySnapshot& snapshot);
void apply_tunnel_status(CoreRegistrySnapshot& snapshot,
                         const exv::core::TunnelStatusSnapshot& status);

CoreRegistrySnapshot core_registry_snapshot_from_hello(
    const nlohmann::json& hello_payload, const std::string& ipc_path);

} // namespace exv::core::lifecycle
