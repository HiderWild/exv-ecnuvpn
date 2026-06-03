#pragma once
#include <string>
#include <functional>
#include <map>

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

    // Built-in action prefixes
    static constexpr const char* VPN_PREFIX = "vpn.";
    static constexpr const char* CONFIG_PREFIX = "config.";
    static constexpr const char* SERVICE_PREFIX = "service.";
    static constexpr const char* ROUTE_PREFIX = "route.";

private:
    std::map<std::string, Handler> handlers_;
};

} // namespace exv::core_api
