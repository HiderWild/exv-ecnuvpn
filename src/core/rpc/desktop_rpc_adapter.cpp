#include "desktop_rpc_adapter.hpp"
#include <nlohmann/json.hpp>

namespace exv::core_api {

DesktopRpcAdapter::DesktopRpcAdapter() = default;

void DesktopRpcAdapter::register_legacy_handler(const std::string& action,
                                                  LegacyHandler handler) {
    // Wrap the legacy handler into the RpcRequest/RpcResponse convention.
    dispatcher_.register_handler(action,
        [h = std::move(handler)](const RpcRequest& req) -> RpcResponse {
            RpcResponse resp;
            try {
                auto payload = nlohmann::json::parse(req.payload_json);
                auto result = h(payload);
                // The legacy handler returns a JSON object directly.
                // If it contains {"ok", false} with an "error" key, treat as failure.
                if (result.is_object() && result.value("ok", true) == false &&
                    result.contains("error")) {
                    resp.success = false;
                    resp.error_message = result.value("error", std::string());
                    resp.error_code = result.value("code", std::string());
                    resp.payload_json = result.dump();
                } else {
                    resp.success = true;
                    resp.payload_json = result.dump();
                }
            } catch (const std::exception& e) {
                resp.success = false;
                resp.error_code = "handler_exception";
                resp.error_message = e.what();
            }
            return resp;
        });
}

nlohmann::json DesktopRpcAdapter::dispatch(const std::string& action,
                                            const nlohmann::json& payload) {
    RpcRequest req;
    req.action = action;
    req.payload_json = payload.dump();

    RpcResponse resp = dispatcher_.dispatch(req);

    // Convert RpcResponse back to legacy JSON format.
    // If the handler was registered via register_legacy_handler, the payload
    // already contains the legacy-format JSON.  For native core_api handlers,
    // we wrap the response into the legacy envelope.
    if (!resp.success && !resp.payload_json.empty()) {
        // The payload may already be a legacy-format error JSON.
        try {
            auto j = nlohmann::json::parse(resp.payload_json);
            if (j.is_object() && j.contains("ok")) {
                return j;  // Already in legacy format.
            }
        } catch (...) {}
    }

    if (!resp.success) {
        nlohmann::json err;
        err["ok"] = false;
        err["error"] = resp.error_message;
        if (!resp.error_code.empty())
            err["code"] = resp.error_code;
        return err;
    }

    try {
        auto j = nlohmann::json::parse(resp.payload_json);
        if (!j.is_object()) {
            return j;
        }
        if (j.contains("ok")) {
            return j;  // Already in legacy format.
        }
        // Wrap native core_api object response into legacy envelope.
        j["ok"] = true;
        return j;
    } catch (...) {
        nlohmann::json result;
        result["ok"] = true;
        result["data"] = resp.payload_json;
        return result;
    }
}

AppRpcDispatcher& DesktopRpcAdapter::dispatcher() {
    return dispatcher_;
}

} // namespace exv::core_api
