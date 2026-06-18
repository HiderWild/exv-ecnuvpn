#pragma once
#include "app_rpc_dispatcher.hpp"
#include "core/lifecycle/core_registry.hpp"
#include "core/lifecycle/core_paths.hpp"

#include <functional>
#include <string>

namespace exv::core_api {

/// Dependency-injected process control for maintenance actions.
/// Production implementations terminate real processes; test
/// implementations record calls without side effects.
struct MaintenanceProcessControl {
    std::function<bool(int pid)> is_pid_alive;
    std::function<bool(int pid)> terminate_pid;
    std::function<bool(const std::string &ipc_path)> try_ipc_shutdown;
    std::function<bool(const std::string &lock_path)> release_lock;
    std::function<bool(const std::string &ipc_path)> is_ipc_available;
};

/// Risk level derived from the registry snapshot.
enum class CoreRiskLevel {
    None,       // idle, no active session
    Active,     // connected or network_ready
    Transition, // connecting, reconnecting, disconnecting, cleaning_up
    Unknown     // corrupt or missing registry
};

/// Risk classification from registry state.
CoreRiskLevel classify_risk(
    const exv::core::lifecycle::CoreRegistryReadResult &registry);

/// Human-readable risk description for UI/CLI prompts.
std::string risk_description(CoreRiskLevel risk);

class MaintenanceActions {
public:
    MaintenanceActions();
    explicit MaintenanceActions(MaintenanceProcessControl control);
    explicit MaintenanceActions(std::string state_dir);
    MaintenanceActions(std::string state_dir,
                       MaintenanceProcessControl control);

    void register_handlers(AppRpcDispatcher &dispatcher);

    RpcResponse inspect_core(const RpcRequest &req);
    RpcResponse kill_stale_core(const RpcRequest &req);

private:
    std::string state_dir_;
    MaintenanceProcessControl control_;
};

} // namespace exv::core_api
