#pragma once
#include <string>
#include <functional>
#include <map>
#include <memory>
#include <vector>

namespace exv::core_api {

struct RpcRequest {
    std::string action;
    std::string payload_json;
    std::string request_id;  // For tracing
};

struct RpcResponse {
    bool success = false;
    std::string payload_json;
    std::string error_code;
    std::string error_message;
    std::string request_id;
};

class AppRpcDispatcher {
public:
    using Handler = std::function<RpcResponse(const RpcRequest&)>;

    void register_handler(const std::string& action, Handler handler);
    RpcResponse dispatch(const RpcRequest& request);

    // Store an action object whose lifetime must match the dispatcher's.
    // Handlers registered by action classes capture `this`; the action object
    // must outlive the dispatcher.  Call this after register_handler to keep
    // the action alive.
    void retain_action(std::shared_ptr<void> action);

    // Built-in action prefixes
    static constexpr const char* VPN_PREFIX = "vpn.";
    static constexpr const char* CONFIG_PREFIX = "config.";
    static constexpr const char* SERVICE_PREFIX = "service.";
    static constexpr const char* ROUTE_PREFIX = "route.";

private:
    std::map<std::string, Handler> handlers_;
    std::vector<std::shared_ptr<void>> retained_actions_;
};

} // namespace exv::core_api
