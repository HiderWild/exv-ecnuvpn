#include "core/rpc/maintenance_actions.hpp"

#include "contracts/generated/system_contract.hpp"
#include "core/lifecycle/core_lock.hpp"
#include "observability/log_facade.hpp"

#include <nlohmann/json.hpp>

#include <exception>
#include <utility>

using json = nlohmann::json;

namespace exv::core_api {
namespace {

bool is_transition_phase(const std::string &phase) {
    static const char *transitions[] = {
        "connecting", "reconnecting", "disconnecting", "cleaning_up"};
    for (const auto *t : transitions) {
        if (phase == t) return true;
    }
    return false;
}

bool requires_confirmation(const RpcRequest &req) {
    try {
        if (req.payload_json.empty()) return false;
        json payload = json::parse(req.payload_json);
        if (payload.is_object() && payload.contains("confirm") &&
            payload["confirm"].is_boolean()) {
            return payload["confirm"].get<bool>();
        }
    } catch (...) {
    }
    return false;
}

} // namespace

CoreRiskLevel classify_risk(
    const exv::core::lifecycle::CoreRegistryReadResult &registry) {
    using ReadState = exv::core::lifecycle::CoreRegistryReadState;

    if (registry.state == ReadState::unknown_state) {
        return CoreRiskLevel::Unknown;
    }
    if (registry.state == ReadState::missing) {
        return CoreRiskLevel::None;
    }
    if (!registry.snapshot.has_value()) {
        return CoreRiskLevel::Unknown;
    }

    const auto &snap = *registry.snapshot;

    if (snap.last_known_connected || snap.last_known_network_ready) {
        return CoreRiskLevel::Active;
    }
    if (is_transition_phase(snap.last_known_tunnel_phase)) {
        return CoreRiskLevel::Transition;
    }
    return CoreRiskLevel::None;
}

std::string risk_description(CoreRiskLevel risk) {
    switch (risk) {
    case CoreRiskLevel::None:
        return "No active VPN session detected.";
    case CoreRiskLevel::Active:
        return "A VPN session or system network resources may still be active. "
               "Terminating the core process may disrupt the active session.";
    case CoreRiskLevel::Transition:
        return "An internal VPN flow may still be in progress. "
               "Terminating the core process may leave the system in an "
               "inconsistent state.";
    case CoreRiskLevel::Unknown:
        return "The core state is unknown. A VPN session or system network "
               "resources may still be active.";
    }
    return "Unknown risk.";
}

MaintenanceActions::MaintenanceActions()
    : MaintenanceActions(
          exv::core::lifecycle::core_registry_path(),
          MaintenanceProcessControl{}) {}

MaintenanceActions::MaintenanceActions(MaintenanceProcessControl control)
    : MaintenanceActions(
          exv::core::lifecycle::core_registry_path(),
          std::move(control)) {}

MaintenanceActions::MaintenanceActions(std::string state_dir)
    : state_dir_(std::move(state_dir)), control_{} {
    if (!control_.is_pid_alive) {
        control_.is_pid_alive = [](int) { return false; };
    }
    if (!control_.terminate_pid) {
        control_.terminate_pid = [](int) { return false; };
    }
    if (!control_.try_ipc_shutdown) {
        control_.try_ipc_shutdown = [](const std::string &) { return false; };
    }
    if (!control_.release_lock) {
        control_.release_lock = [](const std::string &) { return false; };
    }
    if (!control_.is_ipc_available) {
        control_.is_ipc_available = [](const std::string &) { return false; };
    }
}

MaintenanceActions::MaintenanceActions(std::string state_dir,
                                       MaintenanceProcessControl control)
    : state_dir_(std::move(state_dir)),
      control_(std::move(control)) {
    if (!control_.is_pid_alive) {
        control_.is_pid_alive = [](int) { return false; };
    }
    if (!control_.terminate_pid) {
        control_.terminate_pid = [](int) { return false; };
    }
    if (!control_.try_ipc_shutdown) {
        control_.try_ipc_shutdown = [](const std::string &) { return false; };
    }
    if (!control_.release_lock) {
        control_.release_lock = [](const std::string &) { return false; };
    }
    if (!control_.is_ipc_available) {
        control_.is_ipc_available = [](const std::string &) { return false; };
    }
}

void MaintenanceActions::register_handlers(AppRpcDispatcher &dispatcher) {
    dispatcher.register_handler(
        "maintenance.inspectCore",
        [this](const RpcRequest &req) { return inspect_core(req); });
    dispatcher.register_handler(
        "maintenance.killStaleCore",
        [this](const RpcRequest &req) { return kill_stale_core(req); });
}

RpcResponse MaintenanceActions::inspect_core(const RpcRequest &req) {
    (void)req; // inspectCore takes no payload

    const std::string registry_path =
        state_dir_.empty()
            ? exv::core::lifecycle::core_registry_path()
            : exv::core::lifecycle::core_registry_path(state_dir_);

    auto registry =
        exv::core::lifecycle::read_core_registry(registry_path);

    CoreRiskLevel risk = classify_risk(registry);

    json result;
    result["risk_level"] = risk == CoreRiskLevel::None   ? "none"
                         : risk == CoreRiskLevel::Active  ? "active"
                         : risk == CoreRiskLevel::Transition ? "transition"
                                                            : "unknown";
    result["risk_description"] = risk_description(risk);

    if (registry.state ==
            exv::core::lifecycle::CoreRegistryReadState::present &&
        registry.snapshot.has_value()) {
        const auto &snap = *registry.snapshot;
        result["registry"] = {
            {"core_instance_id", snap.core_instance_id},
            {"pid", snap.pid},
            {"core_path", snap.core_path},
            {"ipc_path", snap.ipc_path},
            {"ipc_protocol_version", snap.ipc_protocol_version},
            {"app_version", snap.app_version},
            {"contract_version", snap.contract_version},
            {"started_at", snap.started_at},
            {"last_heartbeat_at", snap.last_heartbeat_at},
            {"last_known_tunnel_phase", snap.last_known_tunnel_phase},
            {"last_known_connected", snap.last_known_connected},
            {"last_known_network_ready", snap.last_known_network_ready},
            {"helper_core_lease_id", snap.helper_core_lease_id},
        };
        result["pid_alive"] = control_.is_pid_alive(snap.pid);
        result["ipc_available"] =
            control_.is_ipc_available(snap.ipc_path);
    } else {
        result["registry_state"] =
            registry.state ==
                    exv::core::lifecycle::CoreRegistryReadState::unknown_state
                ? "unknown"
                : "missing";
    }

    RpcResponse resp;
    resp.success = true;
    resp.payload_json = result.dump();
    return resp;
}

RpcResponse MaintenanceActions::kill_stale_core(const RpcRequest &req) {
    // Destructive action requires confirmation
    if (!requires_confirmation(req)) {
        RpcResponse resp;
        resp.success = false;
        resp.error_code = "confirmation_required";
        resp.error_message =
            "maintenance.killStaleCore requires confirm:true. "
            "Terminating a core process that may have an active VPN session "
            "requires explicit user confirmation.";
        return resp;
    }

    const std::string registry_path =
        state_dir_.empty()
            ? exv::core::lifecycle::core_registry_path()
            : exv::core::lifecycle::core_registry_path(state_dir_);
    const std::string ipc_path =
        state_dir_.empty()
            ? exv::core::lifecycle::core_ipc_path()
            : exv::core::lifecycle::core_ipc_path(state_dir_);
    const std::string lock_path =
        state_dir_.empty()
            ? exv::core::lifecycle::core_lock_path()
            : exv::core::lifecycle::core_lock_path(state_dir_);

    auto registry =
        exv::core::lifecycle::read_core_registry(registry_path);

    CoreRiskLevel risk = classify_risk(registry);
    json result;
    result["risk_level"] = risk == CoreRiskLevel::None   ? "none"
                         : risk == CoreRiskLevel::Active  ? "active"
                         : risk == CoreRiskLevel::Transition ? "transition"
                                                            : "unknown";

    // Prefer IPC shutdown when communication is possible
    bool ipc_shutdown_ok = false;
    if (control_.is_ipc_available(ipc_path)) {
        ipc_shutdown_ok = control_.try_ipc_shutdown(ipc_path);
        result["shutdown_method"] = "ipc";
        result["ipc_shutdown_succeeded"] = ipc_shutdown_ok;
    }

    // If IPC shutdown failed or IPC unavailable, terminate by PID
    if (!ipc_shutdown_ok) {
        int target_pid = 0;
        if (registry.state ==
                exv::core::lifecycle::CoreRegistryReadState::present &&
            registry.snapshot.has_value()) {
            target_pid = registry.snapshot->pid;
        }

        if (target_pid <= 0) {
            RpcResponse resp;
            resp.success = false;
            resp.error_code = "core_not_found";
            resp.error_message =
                "No core process found to terminate.";
            return resp;
        }

        if (!control_.is_pid_alive(target_pid)) {
            // Process already dead — just clean up
            result["shutdown_method"] = "already_dead";
            result["pid"] = target_pid;
        } else {
            bool terminated = control_.terminate_pid(target_pid);
            result["shutdown_method"] = "pid_terminate";
            result["pid"] = target_pid;
            result["terminated"] = terminated;

            if (!terminated) {
                RpcResponse resp;
                resp.success = false;
                resp.error_code = "core_launch_failed";
                resp.error_message =
                    "Failed to terminate core process (pid=" +
                    std::to_string(target_pid) + ").";
                return resp;
            }
        }
    }

    // Verify: PID dead, lock released, IPC unavailable
    bool pid_dead = true;
    bool lock_released = true;
    bool ipc_gone = true;

    if (registry.state ==
            exv::core::lifecycle::CoreRegistryReadState::present &&
        registry.snapshot.has_value()) {
        pid_dead = !control_.is_pid_alive(registry.snapshot->pid);
        lock_released = control_.release_lock(lock_path);
        ipc_gone = !control_.is_ipc_available(ipc_path);
    }

    result["verification"] = {
        {"pid_dead", pid_dead},
        {"lock_released", lock_released},
        {"ipc_unavailable", ipc_gone},
    };

    RpcResponse resp;
    resp.success = true;
    resp.payload_json = result.dump();
    return resp;
}

} // namespace exv::core_api
