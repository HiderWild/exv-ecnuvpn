#include "vpn_actions.hpp"
#include <nlohmann/json.hpp>
#include <core/tunnel_controller.hpp>

using json = nlohmann::json;

namespace exv::core_api {

VpnActions::VpnActions(std::shared_ptr<exv::core::TunnelController> controller)
    : controller_(std::move(controller)) {}

void VpnActions::register_handlers(AppRpcDispatcher& dispatcher) {
    dispatcher.register_handler("vpn.connect",
        [this](const RpcRequest& req) { return connect(req); });
    dispatcher.register_handler("vpn.disconnect",
        [this](const RpcRequest& req) { return disconnect(req); });
    dispatcher.register_handler("vpn.status",
        [this](const RpcRequest& req) { return status(req); });
    dispatcher.register_handler("vpn.set_auto_reconnect",
        [this](const RpcRequest& req) { return set_auto_reconnect(req); });
}

RpcResponse VpnActions::connect(const RpcRequest& req) {
    RpcResponse resp;
    try {
        auto payload = json::parse(req.payload_json);

        exv::core::UserIntent intent;
        intent.desired_connected = true;

        if (payload.contains("profile_id")) {
            intent.profile_id.value = payload["profile_id"].get<std::string>();
        }
        if (payload.contains("auto_reconnect")) {
            intent.auto_reconnect = payload["auto_reconnect"].get<bool>();
        }

        controller_->connect(intent);

        resp.success = true;
        resp.payload_json = json{{"status", "connecting"}}.dump();
    } catch (const std::exception& e) {
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = e.what();
    }
    return resp;
}

RpcResponse VpnActions::disconnect(const RpcRequest& req) {
    RpcResponse resp;
    controller_->disconnect();
    resp.success = true;
    resp.payload_json = json{{"status", "disconnecting"}}.dump();
    return resp;
}

RpcResponse VpnActions::status(const RpcRequest& req) {
    RpcResponse resp;
    auto s = controller_->status();

    auto phase_to_string = [](exv::core::TunnelPhase p) -> const char* {
        switch (p) {
            case exv::core::TunnelPhase::Idle: return "idle";
            case exv::core::TunnelPhase::PreparingHelper: return "preparing_helper";
            case exv::core::TunnelPhase::Authenticating: return "authenticating";
            case exv::core::TunnelPhase::ConnectingCstp: return "connecting_cstp";
            case exv::core::TunnelPhase::ApplyingNetworkConfig: return "applying_network_config";
            case exv::core::TunnelPhase::OpeningPacketDevice: return "opening_packet_device";
            case exv::core::TunnelPhase::Connected: return "connected";
            case exv::core::TunnelPhase::Reconnecting: return "reconnecting";
            case exv::core::TunnelPhase::Disconnecting: return "disconnecting";
            case exv::core::TunnelPhase::CleaningUp: return "cleaning_up";
            case exv::core::TunnelPhase::Failed: return "failed";
            default: return "unknown";
        }
    };

    json result = {
        {"phase", phase_to_string(s.phase)},
        {"desired_connected", s.desired_connected},
        {"auto_reconnect", s.auto_reconnect},
        {"helper_mode", s.helper_mode},
        {"helper_status", s.helper_status},
        {"network_ready", s.network_ready},
        {"server", s.server},
        {"interface_name", s.interface_name}
    };

    if (s.last_error.has_value()) {
        auto& err = s.last_error.value();
        result["last_error"] = {
            {"domain", err.domain},
            {"code", err.code},
            {"message", err.message},
            {"recoverable", err.recoverable},
            {"recommended_action", err.recommended_action}
        };
        if (err.native_code.has_value()) {
            result["last_error"]["native_code"] = err.native_code.value();
        }
        if (!err.native_api.empty()) {
            result["last_error"]["native_api"] = err.native_api;
        }
    }

    if (s.reconnect.has_value()) {
        result["reconnect"] = {
            {"attempt", s.reconnect->attempt},
            {"next_retry_ms", s.reconnect->next_retry_ms}
        };
    }

    resp.success = true;
    resp.payload_json = result.dump();
    return resp;
}

RpcResponse VpnActions::set_auto_reconnect(const RpcRequest& req) {
    RpcResponse resp;
    try {
        auto payload = json::parse(req.payload_json);
        bool enabled = payload.at("enabled").get<bool>();
        controller_->set_auto_reconnect(enabled);
        resp.success = true;
        resp.payload_json = json{{"auto_reconnect", enabled}}.dump();
    } catch (const std::exception& e) {
        resp.success = false;
        resp.error_code = "invalid_payload";
        resp.error_message = e.what();
    }
    return resp;
}

} // namespace exv::core_api
