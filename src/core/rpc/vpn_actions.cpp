#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "core/rpc/vpn_actions.hpp"
#include <nlohmann/json.hpp>
#include "core/tunnel_controller/tunnel_controller.hpp"
#include "core/config/config_manager.hpp"
#include "runtime/runtime_context.hpp"

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
    dispatcher.register_handler("status.get",
        [this](const RpcRequest& req) { return get_legacy_status(req); });
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

    json result = {
        {"phase", exv::core::tunnel_phase_wire_name(s.phase)},
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

RpcResponse VpnActions::get_legacy_status(const RpcRequest& req) {
    RpcResponse resp;
    try {
        ecnuvpn::Config cfg;
        if (ecnuvpn::runtime::is_bootstrapped()) {
            ecnuvpn::config::ConfigManager mgr(ecnuvpn::platform::get_config_dir());
            cfg = mgr.load();
        } else {
            cfg = ecnuvpn::Config{};
        }

        auto snap = controller_ ? controller_->status() : exv::core::TunnelStatusSnapshot{};

        json result;
        result["phase"] = exv::core::tunnel_phase_wire_name(snap.phase);
        result["connected"] = snap.phase == exv::core::TunnelPhase::Connected;
        result["process_running"] = snap.phase != exv::core::TunnelPhase::Idle &&
                                    snap.phase != exv::core::TunnelPhase::Failed;
        result["auto_reconnect"] = snap.auto_reconnect;
        result["server"] = !snap.server.empty() ? snap.server : cfg.server;

        if (snap.last_error.has_value()) {
            const auto& err = snap.last_error.value();
            result["last_error"] = {
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

        resp.success = true;
        resp.payload_json = result.dump();
    } catch (const std::exception& e) {
        resp.success = false;
        resp.error_code = "status_failed";
        resp.error_message = e.what();
    }
    return resp;
}

} // namespace exv::core_api
