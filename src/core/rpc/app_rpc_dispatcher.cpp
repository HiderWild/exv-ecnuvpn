#include "app_rpc_dispatcher.hpp"

#include <stdexcept>
#include <utility>

namespace exv::core_api {

void AppRpcDispatcher::register_handler(const std::string& action, Handler handler) {
    if (handlers_.find(action) != handlers_.end()) {
        throw std::logic_error("Duplicate RPC action handler: " + action);
    }
    handlers_.emplace(action, std::move(handler));
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

} // namespace exv::core_api
