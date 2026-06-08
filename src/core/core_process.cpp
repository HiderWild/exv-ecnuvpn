#include "core_process.hpp"
#include "tunnel_controller.hpp"
#include "reconnect_policy.hpp"
#include "core_api/app_rpc_dispatcher.hpp"
#include "core_api/core_api_setup.hpp"
#include "logger.hpp"
#include "log_renderer.hpp"
#include "core/pipe_ipc.hpp"
#include "runtime/runtime_context.hpp"
#include "config.hpp"
#include "utils.hpp"

#include <nlohmann/json.hpp>

#include <atomic>
#include <csignal>
#include <iostream>
#include <string>
#include <mutex>

using json = nlohmann::json;
using exv::core_api::AppRpcDispatcher;
using exv::core_api::RpcRequest;
using exv::core_api::RpcResponse;

namespace exv::core {

// ------------------------------------------------------------------
// Global signal flag — set by SIGTERM/SIGINT handler
// ------------------------------------------------------------------

static std::atomic<bool> g_stop_requested{false};

static void core_signal_handler(int) {
    g_stop_requested.store(true);
}

// ------------------------------------------------------------------
// Helper: convert TunnelPhase enum to string
// ------------------------------------------------------------------

static const char* phase_to_string(TunnelPhase p) {
    switch (p) {
        case TunnelPhase::Idle:                return "idle";
        case TunnelPhase::PreparingHelper:     return "preparing_helper";
        case TunnelPhase::Authenticating:      return "authenticating";
        case TunnelPhase::ConnectingCstp:      return "connecting_cstp";
        case TunnelPhase::ApplyingNetworkConfig: return "applying_network_config";
        case TunnelPhase::OpeningPacketDevice: return "opening_packet_device";
        case TunnelPhase::Connected:           return "connected";
        case TunnelPhase::Reconnecting:        return "reconnecting";
        case TunnelPhase::Disconnecting:       return "disconnecting";
        case TunnelPhase::CleaningUp:          return "cleaning_up";
        case TunnelPhase::Failed:              return "failed";
        default:                               return "unknown";
    }
}

// ------------------------------------------------------------------
// Helper: build status JSON from TunnelStatusSnapshot
// ------------------------------------------------------------------

static json snapshot_to_json(const TunnelStatusSnapshot& s) {
    json result = {
        {"phase",           phase_to_string(s.phase)},
        {"desired_connected", s.desired_connected},
        {"auto_reconnect",  s.auto_reconnect},
        {"helper_mode",     s.helper_mode},
        {"helper_status",   s.helper_status},
        {"network_ready",   s.network_ready},
        {"server",          s.server},
        {"interface_name",  s.interface_name}
    };

    if (s.last_error.has_value()) {
        auto& err = s.last_error.value();
        json err_json = {
            {"domain",             err.domain},
            {"code",               err.code},
            {"message",            err.message},
            {"recoverable",        err.recoverable},
            {"recommended_action", err.recommended_action}
        };
        if (err.native_code.has_value()) {
            err_json["native_code"] = err.native_code.value();
        }
        if (!err.native_api.empty()) {
            err_json["native_api"] = err.native_api;
        }
        result["last_error"] = std::move(err_json);
    }

    if (s.reconnect.has_value()) {
        result["reconnect"] = {
            {"attempt",       s.reconnect->attempt},
            {"next_retry_ms", s.reconnect->next_retry_ms}
        };
    }

    return result;
}

// ------------------------------------------------------------------
// Helper: write a JSON line to stdout (thread-safe)
// ------------------------------------------------------------------

static std::mutex g_stdout_mutex;

static void write_json_line(const json& obj) {
    std::string line = obj.dump();
    std::lock_guard<std::mutex> lock(g_stdout_mutex);
    std::cout << line << std::endl;
}

// ------------------------------------------------------------------
// Register additional core-process-specific actions that go beyond
// what create_dispatcher() provides.
// ------------------------------------------------------------------

static void register_core_exclusive_actions(
    AppRpcDispatcher& dispatcher,
    std::shared_ptr<TunnelController> controller)
{
    // NOTE: The following actions are already registered by
    //   create_dispatcher() (core_api_setup.cpp) and must NOT be
    //   re-registered here, because register_handler() silently
    //   overwrites the previous handler:
    //
    //   status.get         — VpnActions::get_legacy_status (frontend-compatible shape)
    //   logs.list          — centralized in core_api_setup.cpp
    //   config.getAuth     — ConfigActions::get_auth (reads real config)
    //   config.getSettings — ConfigActions::get_settings (returns settings fields)

    // runtime.status — returns basic runtime information
    dispatcher.register_handler("runtime.status",
        [](const RpcRequest&) -> RpcResponse {
            RpcResponse resp;
            json info;
            info["version"] = ECNUVPN_VERSION;
            info["bootstrapped"] = ecnuvpn::runtime::is_bootstrapped();
            auto paths = ecnuvpn::runtime::paths();
            info["state_dir"] = paths.state_dir;
            info["log_path"] = paths.log_path;
            resp.success = true;
            resp.payload_json = info.dump();
            return resp;
        });

    // service.status — alias for helper_status for convenience
    dispatcher.register_handler("service.status",
        [&dispatcher](const RpcRequest& req) -> RpcResponse {
            RpcRequest aliased = req;
            aliased.action = "service.helper_status";
            return dispatcher.dispatch(aliased);
        });

    // drivers.status — alias for service.driver_status
    dispatcher.register_handler("drivers.status",
        [&dispatcher](const RpcRequest& req) -> RpcResponse {
            RpcRequest aliased = req;
            aliased.action = "service.driver_status";
            return dispatcher.dispatch(aliased);
        });

}

// ------------------------------------------------------------------
// core_process_main
// ------------------------------------------------------------------

int core_process_main(const std::string& config_dir,
                      const std::string& home_dir)
{
    // 1. Bootstrap runtime paths
    ecnuvpn::runtime::bootstrap(config_dir, home_dir);

    // 2. Initialize logger
    ecnuvpn::logger::init();

    // 2b. Instantiate LogRenderer so typed events reach the disk file.
    // Must live for the lifetime of the core process.
    ecnuvpn::LogRenderer log_renderer;

    ecnuvpn::logger::info("Core process starting (mode=core)");

    // 3. Install signal handlers for graceful shutdown
    std::signal(SIGINT,  core_signal_handler);
    std::signal(SIGTERM, core_signal_handler);
#ifndef _WIN32
    // SIGPIPE is irrelevant — we write to stdout, not a pipe
    std::signal(SIGPIPE, SIG_IGN);
#endif

    // 4. Create stub dependencies for TunnelController.
    //
    //    For now we use nullptr for both HelperClient and
    //    PlatformNetworkOps.  TunnelController already handles null
    //    gracefully (it checks helper_ && helper_->is_connected()).
    //    A future iteration will inject real platform implementations.
    auto helper   = std::shared_ptr<exv::helper::HelperClient>(nullptr);
    auto net_ops  = std::shared_ptr<exv::platform::PlatformNetworkOps>(nullptr);

    auto controller = std::make_shared<TunnelController>(
        helper, net_ops, ReconnectConfig{});

    // 5. Register status callback — pushes events to stdout
    controller->set_status_callback([](const TunnelStatusSnapshot& snap) {
        json event = {
            {"event", "status"},
            {"data",  snapshot_to_json(snap)}
        };
        write_json_line(event);
    });

    // 6. Create the RPC dispatcher with all handlers
    auto dispatcher = core_api::create_dispatcher(controller);

    // 7. Register core-process-specific actions
    register_core_exclusive_actions(*dispatcher, controller);

    // 7b. Create pipe listener for CLI connections (also serves as single-instance guard).
    auto pipe_listener = std::make_unique<PipeIpcListener>(core_pipe_path());
    if (!pipe_listener->start()) {
        std::cerr << "fatal: another core process is already running (pipe '"
                  << core_pipe_path() << "' is in use)" << std::endl;
        return 1;
    }

    ecnuvpn::logger::info("Core process ready — reading JSON-RPC from stdin and pipe");

    // 8. Main event loop: read JSON-RPC requests from stdin and pipe, one per line
    std::string line;
    while (!g_stop_requested.load()) {
        if (!std::getline(std::cin, line)) {
            // stdin closed (EOF) — clean exit
            ecnuvpn::logger::info("Core process: stdin EOF, shutting down");
            break;
        }

        // Skip empty lines
        if (line.empty() || line.find_first_not_of(" \t\r\n") == std::string::npos) {
            continue;
        }

        json response;

        try {
            json request = json::parse(line);

            // Extract fields
            int id = 0;
            if (request.contains("id")) {
                id = request["id"].get<int>();
            }

            std::string action;
            if (request.contains("action")) {
                action = request["action"].get<std::string>();
            } else {
                response = {
                    {"id",      id},
                    {"ok",      false},
                    {"code",    "missing_action"},
                    {"message", "Request must contain an 'action' field"}
                };
                write_json_line(response);
                continue;
            }

            std::string payload_json = "{}";
            if (request.contains("payload")) {
                payload_json = request["payload"].dump();
            }

            // Dispatch
            core_api::RpcRequest rpc_req;
            rpc_req.action = action;
            rpc_req.payload_json = payload_json;
            rpc_req.request_id = std::to_string(id);

            core_api::RpcResponse rpc_resp = dispatcher->dispatch(rpc_req);

            // Build wire response
            if (rpc_resp.success) {
                json data = json::parse(rpc_resp.payload_json);
                response = {
                    {"id",   id},
                    {"ok",   true},
                    {"data", data}
                };
            } else {
                response = {
                    {"id",      id},
                    {"ok",      false},
                    {"code",    rpc_resp.error_code},
                    {"message", rpc_resp.error_message}
                };
            }
        } catch (const json::parse_error& e) {
            response = {
                {"id",      0},
                {"ok",      false},
                {"code",    "parse_error"},
                {"message", std::string("Invalid JSON: ") + e.what()}
            };
        } catch (const std::exception& e) {
            response = {
                {"id",      0},
                {"ok",      false},
                {"code",    "internal_error"},
                {"message", e.what()}
            };
        }

        write_json_line(response);

        // Process CLI pipe connections (non-blocking)
        pipe_listener->accept_one([&dispatcher](const std::string& request_line) -> std::string {
            try {
                json req = json::parse(request_line);
                int id = req.value("id", 0);
                std::string action = req.value("action", "");
                if (action.empty()) {
                    json err = {{"id", id}, {"ok", false}, {"code", "missing_action"}, {"message", "Request must contain an 'action' field"}};
                    return err.dump();
                }
                std::string payload_json = req.contains("payload") ? req["payload"].dump() : "{}";
                core_api::RpcRequest rpc_req;
                rpc_req.action = action;
                rpc_req.payload_json = payload_json;
                rpc_req.request_id = std::to_string(id);
                core_api::RpcResponse rpc_resp = dispatcher->dispatch(rpc_req);
                if (rpc_resp.success) {
                    json data = json::parse(rpc_resp.payload_json);
                    json resp = {{"id", id}, {"ok", true}, {"data", data}};
                    return resp.dump();
                } else {
                    json resp = {{"id", id}, {"ok", false}, {"code", rpc_resp.error_code}, {"message", rpc_resp.error_message}};
                    return resp.dump();
                }
            } catch (const std::exception& e) {
                json err = {{"id", 0}, {"ok", false}, {"code", "parse_error"}, {"message", std::string("Invalid JSON: ") + e.what()}};
                return err.dump();
            }
        });
    }

    ecnuvpn::logger::info("Core process shutting down");
    return 0;
}

} // namespace exv::core
