#pragma once
#include "app_rpc_dispatcher.hpp"
#include <nlohmann/json.hpp>
#include <functional>
#include <optional>
#include <string>

namespace exv::core_api {

/// Adapter that bridges the legacy app_api::handle_action(string, json)
/// interface to the new AppRpcDispatcher.  Each desktop action is registered
/// as a dispatcher handler; the adapter converts between the two calling
/// conventions.
///
/// The long-term goal (A1 in the architecture compliance plan) is to move
/// all action logic into core_api/*_actions classes and make app_api.cpp a
/// thin shim that just calls DesktopRpcAdapter::dispatch().
class DesktopRpcAdapter {
public:
    using LegacyHandler = std::function<nlohmann::json(const nlohmann::json&)>;

    DesktopRpcAdapter();

    /// Register a legacy handler that takes (action, payload) and returns json.
    /// The adapter wraps it into the RpcRequest/RpcResponse convention.
    void register_legacy_handler(
        const std::string& action,
        LegacyHandler handler,
        std::optional<RpcActionMetadata> metadata = std::nullopt);

    /// Dispatch a desktop action through the registered handlers.
    /// Returns the JSON response in the legacy format (for backward compat).
    nlohmann::json dispatch(const std::string& action,
                            const nlohmann::json& payload);

    /// Access the underlying dispatcher for registering core_api action classes.
    AppRpcDispatcher& dispatcher();

private:
    AppRpcDispatcher dispatcher_;
};

} // namespace exv::core_api
