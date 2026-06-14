#include "helper/helper.hpp"
#include "helper/helper_ipc.hpp"

#include "helper/common/helper_messages.hpp"
#include "helper/helper_v2_handler.hpp"
#include "logger.hpp"
#include "runtime/runtime_context.hpp"
#include "feedback/feedback.hpp"
#include "helper/platform/helper_client.hpp"
#include "helper/platform/helper_lifecycle.hpp"
#include "helper/platform/helper_platform.hpp"
#include "helper/platform/helper_service_manager.hpp"
#include "utils.hpp"
#include "vpn.hpp"

#include <cerrno>
#include <csignal>
#include <cstring>
#include <chrono>
#include <iostream>
#include <sstream>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace ecnuvpn {
namespace helper {

namespace {

volatile sig_atomic_t daemon_stop_requested = 0;
DaemonOptions active_daemon_options;

#if defined(ECNUVPN_PLATFORM_WINDOWS)
inline constexpr std::string_view kHelperPlatformName = "windows";
inline constexpr std::string_view kHelperTransportName = "named-pipe";
#elif defined(ECNUVPN_PLATFORM_DARWIN)
inline constexpr std::string_view kHelperPlatformName = "darwin";
inline constexpr std::string_view kHelperTransportName = "unix-socket";
#elif defined(ECNUVPN_PLATFORM_LINUX)
inline constexpr std::string_view kHelperPlatformName = "linux";
inline constexpr std::string_view kHelperTransportName = "unix-socket";
#else
#error "Unsupported ECNU-VPN platform"
#endif

void daemon_signal_handler(int) {
  daemon_stop_requested = 1;
}

nlohmann::json make_error(const std::string &message,
                          const std::string &code = std::string()) {
  return feedback::make_error(message, code);
}

nlohmann::json make_helper_capabilities() {
  return nlohmann::json{{"vpn_connect", true},
                        {"vpn_disconnect", true},
                        {"logs", false},
                        {"events", false},
                        {"temporary_connect", active_daemon_options.oneshot},
                        {"oneshot_mode", active_daemon_options.oneshot},
                        {"service_mode", true}};
}

nlohmann::json make_helper_descriptor() {
  const auto &platform_config = platform::helper_platform_config();
  return nlohmann::json{{"name", "exv-helper"},
                        {"version", ECNUVPN_VERSION},
                        {"platform_service_mode", platform_config.service_mode},
                        {"mode", active_daemon_options.mode},
                        {"endpoint", active_daemon_options.endpoint},
                        {"auth_required", active_daemon_options.auth_required},
                        {"platform", std::string(kHelperPlatformName)},
                        {"transport", std::string(kHelperTransportName)},
                        {"capabilities", make_helper_capabilities()}};
}

nlohmann::json make_hello_response() {
  nlohmann::json descriptor = make_helper_descriptor();
  descriptor["ok"] = true;
  return descriptor;
}

void add_helper_descriptor_fields(nlohmann::json &response) {
  nlohmann::json descriptor = make_helper_descriptor();
  response["helper"] = descriptor;
  response["backend_mode"] = descriptor.value("mode", "service");
  response["transport"] = descriptor.value("transport", "unknown");
  response["capabilities"] =
      descriptor.value("capabilities", nlohmann::json::object());
}

bool send_request(const nlohmann::json &request, nlohmann::json *response,
                  std::string *error_message = nullptr,
                  int /*timeout_seconds*/ = 15) {
  nlohmann::json result = platform::send_helper_request(request);
  if (!result.is_object()) {
    if (error_message) *error_message = "Failed to parse EXV helper response.";
    return false;
  }
  if (result.contains("ok") && result["ok"].is_boolean() && !result["ok"].get<bool>()) {
    if (result.contains("code") && result["code"].is_string() &&
        result["code"].get<std::string>() == std::string(platform::kHelperUnavailableCode)) {
      if (error_message) *error_message = "EXV helper is not available.";
    } else {
      if (error_message)
        *error_message = result.value("message", std::string("EXV helper request failed."));
    }
    return false;
  }
  if (response) *response = result;
  return true;
}

bool wait_until_available(int attempts = 1, unsigned int delay_us = 0) {
  for (int i = 0; i < attempts; ++i) {
    nlohmann::json response;
    std::string error_message;
    if (send_request(nlohmann::json{{"action", "status"}}, &response, &error_message)) {
      return true;
    }
    if (i + 1 < attempts && delay_us > 0) {
      platform::sleep_ms(static_cast<int>(delay_us / 1000));
    }
  }
  return false;
}

void reap_finished_request_handlers() {
  platform::reap_children();
}

bool wait_until_available_for_platform(int attempts, unsigned int delay_us) {
  return wait_until_available(attempts, delay_us);
}

bool send_request_for_platform(const nlohmann::json &request,
                               nlohmann::json *response,
                               std::string *error_message,
                               int timeout_seconds) {
  return send_request(request, response, error_message, timeout_seconds);
}

platform::HelperServiceManagerContext make_helper_service_manager_context() {
  return platform::HelperServiceManagerContext{
      wait_until_available_for_platform,
      send_request_for_platform,
      nullptr,
  };
}

} // namespace

void request_daemon_stop() {
  daemon_stop_requested = 1;
  platform::wake_helper_daemon_for_shutdown();
}

bool is_available() {
  return wait_until_available();
}

int install_service(const std::string &executable_path) {
  return platform::install_helper_service(
      executable_path, make_helper_service_manager_context());
}

int uninstall_service() {
  return platform::uninstall_helper_service(
      make_helper_service_manager_context());
}

int show_service_status() {
  return platform::show_helper_service_status(
      make_helper_service_manager_context());
}

int worker_main(const std::string &request_path) {
  // V1 legacy worker: reads request from file, delegates to vpn::start_with_password.
  // Preserved for backward compatibility with legacy_openconnect engine.
  try {
    std::string content = utils::read_file(request_path);
    nlohmann::json request = nlohmann::json::parse(content);
    Config cfg = request.at("config").get<Config>();
    std::string password = request.at("password").get<std::string>();
    int retry_limit = request.value("retry_limit", 0);

    runtime::bootstrap(request.value("config_dir", std::string()),
                       request.value("home", std::string()), true);
    logger::init();

    return vpn::start(cfg, retry_limit);
  } catch (const std::exception &e) {
    logger::error(std::string("worker_main failed: ") + e.what());
    return 1;
  }
}

int daemon_main(const DaemonOptions &options) {
  daemon_stop_requested = 0;
  active_daemon_options = options;

  signal(SIGTERM, daemon_signal_handler);
  signal(SIGINT, daemon_signal_handler);

  logger::info("Helper daemon starting (mode=" + options.mode + ")");

  // Debug: log the exact endpoint being listened on
  std::cerr << "[DEBUG] Helper listening on: " << options.endpoint << std::endl;

  auto ipc = helper::create_ipc_server();
  if (!ipc || !ipc->start(options.endpoint)) {
    logger::error("Failed to open helper IPC endpoint: " + options.endpoint);
    return 1;
  }

  // Create V2 handler
  auto v2_handler = std::make_unique<exv::helper::HelperV2Handler>();

  logger::info("Helper daemon ready, accepting connections");

  while (!daemon_stop_requested) {
    if (!ipc->accept_client()) {
      if (daemon_stop_requested)
        break;
      reap_finished_request_handlers();
      v2_handler->tick();
      continue;
    }

    if (!ipc->verify_client()) {
      ipc->close_client();
      continue;
    }

    while (!daemon_stop_requested) {
      std::string raw = ipc->read_request();
      if (raw.empty())
        break;

      // Parse and dispatch
      try {
        nlohmann::json req = nlohmann::json::parse(raw);

        if (req.contains("op")) {
          exv::helper::HelperRequest v2_req = exv::helper::helper_request_from_json(req);
          exv::helper::HelperResponse v2_resp = v2_handler->handle(v2_req);
          nlohmann::json resp_json = v2_resp;
          ipc->send_response(resp_json.dump());
        } else {
          std::string action = req.value("action", std::string());
          if (action == "hello") {
            ipc->send_response(make_hello_response().dump());
          } else if (action == "status") {
            nlohmann::json resp = make_hello_response();
            resp["running"] = true;
            ipc->send_response(resp.dump());
          } else {
            nlohmann::json err = make_error("Unknown action: " + action, "unknown_action");
            ipc->send_response(err.dump());
          }
        }
      } catch (const std::exception &e) {
        nlohmann::json err = make_error(std::string("Parse error: ") + e.what());
        ipc->send_response(err.dump());
      }

      reap_finished_request_handlers();
      v2_handler->tick();
    }

    ipc->close_client();
    reap_finished_request_handlers();
    v2_handler->tick();
  }

  logger::info("Helper daemon shutting down");
  ipc->close();
  platform::cleanup_daemon_endpoint(active_daemon_options.endpoint);
  return 0;
}

int daemon_main() {
  DaemonOptions options;
  options.mode = "service";
  options.endpoint = platform::helper_platform_config().endpoint;
  return daemon_main(options);
}

} // namespace helper
} // namespace ecnuvpn
