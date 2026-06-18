#pragma once

#include "core/rpc/app_rpc_dispatcher.hpp"
#include "core/rpc/rpc_action_metadata.hpp"

#include <functional>
#include <memory>

namespace exv::core_api {

using RpcResponseCallback = std::function<void(RpcResponse)>;

struct LaneWorkItem {
    RpcRequest request;
    RpcActionMetadata metadata;
    std::function<RpcResponse(const RpcRequest&)> handler;
    RpcResponseCallback respond;
};

class LaneScheduler {
public:
    LaneScheduler();
    ~LaneScheduler();

    LaneScheduler(const LaneScheduler&) = delete;
    LaneScheduler& operator=(const LaneScheduler&) = delete;

    bool start();
    void stop();
    bool schedule(LaneWorkItem item);

private:
    struct LaneState;

    LaneState& state_for(RpcLane lane);

    std::unique_ptr<LaneState> control_;
    std::unique_ptr<LaneState> read_model_;
    std::unique_ptr<LaneState> vpn_control_;
    std::unique_ptr<LaneState> config_store_;
    std::unique_ptr<LaneState> diagnostics_;
    std::unique_ptr<LaneState> platform_admin_;
};

} // namespace exv::core_api
