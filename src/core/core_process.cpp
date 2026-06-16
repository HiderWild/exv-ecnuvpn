#include "core_process.hpp"
#include "core/lifecycle/core_lock.hpp"
#include "core/lifecycle/core_paths.hpp"
#include "core/lifecycle/core_registry.hpp"
#include "observability/log_facade.hpp"
#include "platform/common/logging/log_runtime.hpp"
#include "common/diagnostics/log_renderer.hpp"
#include "core/pipe_ipc.hpp"
#include "runtime/runtime_context.hpp"
#include "core/app_api/app_api.hpp"
#include "core/rpc/core_api_setup.hpp"
#include "core/tunnel_controller/reconnect_policy.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"

#include <nlohmann/json.hpp>

#include <atomic>
#include <csignal>
#include <iostream>
#include <chrono>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#ifdef _WIN32
#include <io.h>
#include <fcntl.h>
#endif
#ifndef _WIN32
#include <sys/stat.h>
#include <unistd.h>
#endif

using json = nlohmann::json;

namespace exv::core {

// ------------------------------------------------------------------
// Global signal flag — set by SIGTERM/SIGINT handler
// ------------------------------------------------------------------

static std::atomic<bool> g_stop_requested{false};

static void core_signal_handler(int) {
    g_stop_requested.store(true);
}

// ------------------------------------------------------------------
// Helper: write a JSON line to stdout (thread-safe)
// ------------------------------------------------------------------

static std::mutex g_stdout_mutex;
static std::mutex g_desktop_dispatch_mutex;

static void write_json_line(const json& obj) {
    std::string line = obj.dump();
    std::lock_guard<std::mutex> lock(g_stdout_mutex);
    std::cout << line << '\n' << std::flush;
}

// ------------------------------------------------------------------
// Helper: dispatch an Electron-facing desktop action through the real
// app_api handlers and convert the legacy result to core JSON-RPC wire.
// ------------------------------------------------------------------

static json desktop_action_response(int id,
                                    const std::string& action,
                                    const json& payload) {
    json result;
    {
        std::lock_guard<std::mutex> lock(g_desktop_dispatch_mutex);
        result = ecnuvpn::app_api::handle_action(action, payload);
    }

    if (result.is_object() && result.value("ok", true) == false) {
        std::string message = result.value("error", std::string());
        if (message.empty()) {
            message = result.value("message", std::string());
        }
        if (message.empty()) {
            message = "Desktop action failed";
        }

        json response = {
            {"id", id},
            {"ok", false},
            {"code", result.value("code", std::string())},
            {"message", message}
        };
        return response;
    }

    json data = result;
    if (data.is_object() && data.value("ok", false) == true) {
        data.erase("ok");
    }

    return {
        {"id", id},
        {"ok", true},
        {"data", data}
    };
}

static json handle_desktop_request_json(const json& request) {
    int id = request.value("id", 0);
    std::string action = request.value("action", std::string());
    if (action.empty()) {
        return {
            {"id", id},
            {"ok", false},
            {"code", "missing_action"},
            {"message", "Request must contain an 'action' field"}
        };
    }

    const json payload = request.contains("payload")
        ? request.at("payload")
        : json::object();
    return desktop_action_response(id, action, payload);
}

static bool is_native_core_request(const json& request) {
    return request.contains("request_id") || request.contains("payload_json") ||
           request.value("envelope", std::string()) == "core";
}

static json native_core_response(exv::core_api::AppRpcDispatcher& dispatcher,
                                 const json& request) {
    std::string action = request.value("action", std::string());
    std::string request_id = request.value("request_id", std::string());
    if (action.empty()) {
        return {
            {"request_id", request_id},
            {"success", false},
            {"error_code", "missing_action"},
            {"error_message", "Request must contain an 'action' field"}
        };
    }

    exv::core_api::RpcRequest rpc_request;
    rpc_request.action = action;
    rpc_request.request_id = request_id;
    if (request.contains("payload_json")) {
        const json& payload_json = request.at("payload_json");
        rpc_request.payload_json =
            payload_json.is_string() ? payload_json.get<std::string>()
                                     : payload_json.dump();
    } else if (request.contains("payload")) {
        rpc_request.payload_json = request.at("payload").dump();
    } else {
        rpc_request.payload_json = "{}";
    }

    exv::core_api::RpcResponse rpc_response =
        dispatcher.dispatch(rpc_request);

    json response = {
        {"request_id", rpc_response.request_id},
        {"success", rpc_response.success}
    };
    if (rpc_response.success) {
        response["payload_json"] =
            rpc_response.payload_json.empty() ? "{}" : rpc_response.payload_json;
    } else {
        response["error_code"] = rpc_response.error_code;
        response["error_message"] = rpc_response.error_message;
    }
    return response;
}

// Supported request envelopes:
// - Desktop envelope: id/action/payload -> id/ok/data or id/ok/code/message.
//   This keeps Electron and CLI pipe clients on the legacy app_api surface.
// - Core native envelope: action/payload_json/request_id ->
//   success/payload_json or error_code/error_message.  This routes through
//   AppRpcDispatcher while sharing the same use-case services as desktop.
static json handle_request_json(exv::core_api::AppRpcDispatcher& dispatcher,
                                const json& request) {
    if (is_native_core_request(request)) {
        return native_core_response(dispatcher, request);
    }
    return handle_desktop_request_json(request);
}

static std::string handle_request_line(exv::core_api::AppRpcDispatcher& dispatcher,
                                       const std::string& request_line) {
    try {
        return handle_request_json(dispatcher, json::parse(request_line)).dump();
    } catch (const json::parse_error& e) {
        json err = {
            {"id", 0},
            {"ok", false},
            {"code", "parse_error"},
            {"message", std::string("Invalid JSON: ") + e.what()}
        };
        return err.dump();
    } catch (const std::exception& e) {
        json err = {
            {"id", 0},
            {"ok", false},
            {"code", "internal_error"},
            {"message", e.what()}
        };
        return err.dump();
    }
}

static std::optional<exv::core::lifecycle::CoreRegistrySnapshot>
bootstrap_registry_snapshot(exv::core_api::AppRpcDispatcher& dispatcher,
                            const std::string& pipe_path) {
    exv::core_api::RpcRequest request;
    request.action = "core.hello";
    request.request_id = "core-process-bootstrap";
    request.payload_json = "{}";

    auto response = dispatcher.dispatch(request);
    if (!response.success) {
        return std::nullopt;
    }

    try {
        return exv::core::lifecycle::core_registry_snapshot_from_hello(
            json::parse(response.payload_json), pipe_path);
    } catch (...) {
        return std::nullopt;
    }
}

// ------------------------------------------------------------------
// core_process_main
// ------------------------------------------------------------------

int core_process_main(const std::string& config_dir,
                      const std::string& home_dir,
                      bool use_stdin)
{
    g_stop_requested.store(false);

    // 0. Ensure stdout/stdin are in text mode and unbuffered for pipe communication
#ifdef _WIN32
    // Use text mode (not binary) for proper line handling with Node.js pipes
    _setmode(_fileno(stdout), _O_TEXT);
    _setmode(_fileno(stdin), _O_TEXT);
    // Also set stderr to text mode for debug output
    _setmode(_fileno(stderr), _O_TEXT);
#endif
    // Keep the current C++ stream buffers intact; lifecycle tests replace
    // std::cin/std::cout rdbufs to exercise core_process_main() in-process.
    // Enable unbuffered output - critical for real-time pipe communication
    std::cout.setf(std::ios::unitbuf);
    // Also unbuffer stderr for immediate debug output
    std::cerr.setf(std::ios::unitbuf);

    // 1. Bootstrap runtime paths
    ecnuvpn::runtime::bootstrap(config_dir, home_dir);

    // 2. Initialize logger
    ecnuvpn::platform::logging::configure_default_logging(false);

    // 2b. Instantiate LogRenderer so typed events reach the disk file.
    // Must live for the lifetime of the core process.
    ecnuvpn::LogRenderer log_renderer;

    exv::observability::LogFacade::info("Core process starting (mode=core, use_stdin=" + std::string(use_stdin ? "true" : "false") + ")");

    // 3. Install signal handlers for graceful shutdown
    std::signal(SIGINT,  core_signal_handler);
    std::signal(SIGTERM, core_signal_handler);
#ifndef _WIN32
    // SIGPIPE is irrelevant — we write to stdout, not a pipe
    std::signal(SIGPIPE, SIG_IGN);
#endif

    auto core_lock = exv::core::lifecycle::CoreInstanceLock::try_acquire();
    if (!core_lock.has_value()) {
        std::cerr << "fatal: another core process already owns the versioned core lock"
                  << std::endl;
        return 1;
    }

    auto controller = std::make_shared<TunnelController>(
        std::shared_ptr<exv::helper::HelperClient>{},
        std::shared_ptr<exv::platform::PlatformNetworkOps>{},
        ReconnectConfig{});
    auto native_dispatcher = exv::core_api::create_dispatcher(controller);
    auto dispatch_line = [&](const std::string& request_line) {
        return handle_request_line(*native_dispatcher, request_line);
    };

    const auto pipe_path = core_pipe_path();

    // 5. Create pipe listener for CLI connections after owning the versioned lock.
    auto pipe_listener = std::make_unique<PipeIpcListener>(pipe_path);
    if (!pipe_listener->start()) {
        std::cerr << "fatal: another core process is already running (pipe '"
                  << pipe_path << "' is in use)" << std::endl;
        return 1;
    }

    auto registry_snapshot = bootstrap_registry_snapshot(*native_dispatcher, pipe_path);
    if (!registry_snapshot.has_value()) {
        std::cerr << "fatal: could not initialize versioned core registry"
                  << std::endl;
        pipe_listener->stop();
        return 1;
    }

    exv::core::lifecycle::apply_tunnel_status(*registry_snapshot,
                                              controller->status());
    exv::core::lifecycle::refresh_core_registry_heartbeat(*registry_snapshot);

    const auto registry_path = exv::core::lifecycle::core_registry_path();
    std::mutex registry_mutex;
    auto persist_registry = [&] {
        exv::core::lifecycle::CoreRegistrySnapshot snapshot_copy;
        {
            std::lock_guard<std::mutex> lock(registry_mutex);
            exv::core::lifecycle::refresh_core_registry_heartbeat(*registry_snapshot);
            snapshot_copy = *registry_snapshot;
        }
        return exv::core::lifecycle::write_core_registry(snapshot_copy,
                                                         registry_path);
    };

    controller->set_status_callback([&](const TunnelStatusSnapshot& status) {
        exv::core::lifecycle::CoreRegistrySnapshot snapshot_copy;
        {
            std::lock_guard<std::mutex> lock(registry_mutex);
            exv::core::lifecycle::apply_tunnel_status(*registry_snapshot, status);
            exv::core::lifecycle::refresh_core_registry_heartbeat(*registry_snapshot);
            snapshot_copy = *registry_snapshot;
        }
        (void)exv::core::lifecycle::write_core_registry(snapshot_copy,
                                                        registry_path);
    });

    if (!persist_registry()) {
        exv::observability::LogFacade::warn(
            "Core process could not persist the versioned core registry");
    }

    std::atomic<bool> registry_worker_stop{false};
    std::thread registry_worker([&] {
        while (!registry_worker_stop.load() && !g_stop_requested.load()) {
            std::this_thread::sleep_for(std::chrono::seconds(1));
            if (registry_worker_stop.load() || g_stop_requested.load()) {
                break;
            }
            (void)persist_registry();
        }
    });

    exv::observability::LogFacade::info(
        "Core process ready — reading JSON-RPC from stdin and pipe");

    // 8. Main event loop: read JSON-RPC requests from stdin and pipe, one per line
    std::string line;

    // Daemon mode (pipe-only) when use_stdin=false
    bool stdin_available = use_stdin;

    std::atomic<bool> pipe_worker_stop{false};
    std::thread pipe_worker;

    if (!stdin_available) {
        exv::observability::LogFacade::info("Core process: running in daemon mode (pipe-only)");
    } else {
        pipe_worker = std::thread([&] {
            while (!pipe_worker_stop.load() && !g_stop_requested.load()) {
                pipe_listener->accept_one(dispatch_line);
                std::this_thread::sleep_for(std::chrono::milliseconds(10));
            }
        });
    }

    while (!g_stop_requested.load()) {
        // In daemon mode, just process pipe connections in a loop
        if (!stdin_available) {
            pipe_listener->accept_one(dispatch_line);
            // Sleep briefly to avoid busy-waiting
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
            continue;
        }

        if (!std::getline(std::cin, line)) {
            exv::observability::LogFacade::info("Core process: stdin EOF, shutting down");
            break;
        }


        // Skip empty lines
        if (line.empty() || line.find_first_not_of(" \t\r\n") == std::string::npos) {
            // Process pipe connections even on empty lines
            pipe_listener->accept_one(dispatch_line);
            continue;
        }

        json response;

        try {
            json request = json::parse(line);
            response = handle_request_json(*native_dispatcher, request);
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

        // Note: In stdin mode, we don't process pipe connections here.
        // Pipe processing is only needed in daemon mode (handled above in the !stdin_available block).
        // Calling accept_one() here would block the stdin read loop.
    }

    registry_worker_stop.store(true);
    controller->set_status_callback({});
    if (registry_worker.joinable()) {
        registry_worker.join();
    }

    pipe_worker_stop.store(true);
    if (pipe_worker.joinable()) {
        pipe_worker.join();
    }
    pipe_listener->stop();

    exv::core::lifecycle::CoreRegistryDeleteMatch delete_match;
    {
        std::lock_guard<std::mutex> lock(registry_mutex);
        delete_match =
            exv::core::lifecycle::core_registry_delete_match(*registry_snapshot);
    }
    (void)exv::core::lifecycle::compare_and_delete_core_registry(
        registry_path, delete_match);

    exv::observability::LogFacade::info("Core process shutting down");
    return 0;
}

} // namespace exv::core
