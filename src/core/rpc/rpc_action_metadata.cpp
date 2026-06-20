#include "core/rpc/rpc_action_metadata.hpp"

#include <initializer_list>

namespace exv::core_api {
namespace {

bool is_one_of(std::string_view action,
               std::initializer_list<std::string_view> values) {
    for (std::string_view value : values) {
        if (action == value) {
            return true;
        }
    }
    return false;
}

RpcActionMetadata metadata(RpcLane lane,
                           RpcConflictClass conflict = RpcConflictClass::None,
                           bool mutates_state = false) {
    RpcActionMetadata out;
    out.lane = lane;
    out.conflict = conflict;
    out.mutates_state = mutates_state;
    return out;
}

} // namespace

RpcActionMetadata default_metadata_for_action(std::string_view action) {
    if (is_one_of(action, {"core.hello", "core.shutdown", "window.setMode",
                           "window.resolveClosePrompt"})) {
        return metadata(RpcLane::Control);
    }

    if (is_one_of(action, {"status.get", "vpn.status", "runtime.status",
                           "service.status", "helper.status",
                           "drivers.status", "key.status",
                           "cli.status", "maintenance.inspectCore"})) {
        return metadata(RpcLane::ReadModel);
    }

    if (is_one_of(action, {"vpn.connect", "vpn.disconnect",
                           "vpn.authInteraction.get",
                           "vpn.authInteraction.respond"})) {
        return metadata(RpcLane::VpnControl,
                        RpcConflictClass::VpnWorkflowIntent,
                        true);
    }

    if (is_one_of(action, {"config.get", "config.getAuth",
                           "config.saveAuth", "config.getSettings",
                           "config.saveSettings", "config.getKey",
                           "config.reset", "config.import", "config.export",
                           "routes.list", "routes.add", "routes.remove",
                           "routes.reset", "key.reset"})) {
        const bool writes = !is_one_of(action, {"config.get", "config.getAuth",
                                                "config.getSettings",
                                                "config.getKey",
                                                "routes.list"});
        return metadata(RpcLane::ConfigStore,
                        writes ? RpcConflictClass::ConfigWrite
                               : RpcConflictClass::None,
                        writes);
    }

    if (action == "logs.list") {
        return metadata(RpcLane::Diagnostics);
    }

    if (is_one_of(action, {"service.install", "service.uninstall",
                           "cli.install", "cli.uninstall",
                           "drivers.install", "maintenance.killStaleCore"})) {
        return metadata(RpcLane::PlatformAdmin,
                        RpcConflictClass::PlatformAdminWrite,
                        true);
    }

    return metadata(RpcLane::ReadModel);
}

std::string_view lane_name(RpcLane lane) {
    switch (lane) {
    case RpcLane::Control:
        return "control";
    case RpcLane::ReadModel:
        return "read_model";
    case RpcLane::VpnControl:
        return "vpn_control";
    case RpcLane::ConfigStore:
        return "config_store";
    case RpcLane::Diagnostics:
        return "diagnostics";
    case RpcLane::PlatformAdmin:
        return "platform_admin";
    }
    return "unknown";
}

} // namespace exv::core_api
