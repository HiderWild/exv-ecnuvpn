#pragma once

#include <string_view>

namespace exv::core_api {

enum class RpcLane {
    Control,
    ReadModel,
    VpnControl,
    ConfigStore,
    Diagnostics,
    PlatformAdmin,
};

enum class RpcConflictClass {
    None,
    VpnWorkflowIntent,
    ConfigWrite,
    PlatformAdminWrite,
};

struct RpcActionMetadata {
    RpcLane lane = RpcLane::ReadModel;
    RpcConflictClass conflict = RpcConflictClass::None;
    bool mutates_state = false;
};

RpcActionMetadata default_metadata_for_action(std::string_view action);
std::string_view lane_name(RpcLane lane);

} // namespace exv::core_api
