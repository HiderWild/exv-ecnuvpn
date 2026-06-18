#include "app_rpc_dispatcher.hpp"

#include <stdexcept>
#include <utility>

namespace exv::core_api {

void AppRpcDispatcher::register_handler(const std::string& action, Handler handler) {
    register_handler(action, std::move(handler), default_metadata_for_action(action));
}

void AppRpcDispatcher::register_handler(const std::string& action,
                                        Handler handler,
                                        RpcActionMetadata metadata) {
    if (handlers_.find(action) != handlers_.end()) {
        throw std::logic_error("Duplicate RPC action handler: " + action);
    }
    handlers_.emplace(action, std::move(handler));
    metadata_.emplace(action, metadata);
}

RpcResponse AppRpcDispatcher::dispatch(const RpcRequest& request) {
    auto it = handlers_.find(request.action);
    if (it == handlers_.end()) {
        RpcResponse resp;
        resp.success = false;
        resp.error_code = "unknown_action";
        resp.error_message = "No handler registered for action: " + request.action;
        resp.request_id = request.request_id;
        return resp;
    }
    RpcResponse resp = it->second(request);
    resp.request_id = request.request_id;
    return resp;
}

void AppRpcDispatcher::retain_action(std::shared_ptr<void> action) {
    retained_actions_.push_back(std::move(action));
}

std::optional<RpcActionMetadata>
AppRpcDispatcher::metadata_for(std::string_view action) const {
    auto it = metadata_.find(action);
    if (it == metadata_.end()) {
        return std::nullopt;
    }
    return it->second;
}

} // namespace exv::core_api
