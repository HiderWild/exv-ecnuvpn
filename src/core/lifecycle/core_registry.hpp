#pragma once

#include "core/tunnel_controller/tunnel_state.hpp"
#include "platform/common/core_lifecycle_contract.hpp"

#include <nlohmann/json.hpp>

#include <string>

namespace exv::core::lifecycle {

struct CoreRegistryDeleteMatch {
    std::string core_instance_id;
    int pid = 0;
    std::string helper_core_lease_id;
    std::string ipc_protocol_version;
};

bool write_core_registry(const CoreRegistrySnapshot& snapshot);
bool write_core_registry(const CoreRegistrySnapshot& snapshot,
                         const std::string& registry_path);

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

nlohmann::json core_registry_to_json(const CoreRegistrySnapshot& snapshot);

} // namespace exv::core::lifecycle
