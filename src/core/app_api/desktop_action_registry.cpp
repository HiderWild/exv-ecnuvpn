#include "core/app_api/desktop_action_registry.hpp"

#include "core/app_api/desktop_json.hpp"
#include "core/app_api/desktop_runtime_context.hpp"
#include "core/app_api/desktop_status_presenter.hpp"
#include "core/app_api/desktop_tunnel_host.hpp"
#include "core/config/config.hpp"
#include "core/config/config_api.hpp"
#include "core/config/config_manager.hpp"
#include "core/config/config_platform_view.hpp"
#include "core/connection/connection_attempt.hpp"
#include "core/tunnel_controller/timing.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "core/crypto/crypto.hpp"
#include "feedback/feedback.hpp"
#include "helper/helper.hpp"
#include "common/diagnostics/logger.hpp"
#include "platform/common/app_api_runtime_policy.hpp"
#include "platform/common/backend_resolver.hpp"
#include "platform/common/helper_client.hpp"
#include "platform/common/driver_status.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/oneshot_bootstrap.hpp"
#include "platform/common/path_utils.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/process_control.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/runtime_status.hpp"
#include "platform/common/service_status.hpp"
#include "runtime/runtime_context.hpp"
#include "core/tunnel_controller/tunnel_controller_active.hpp"
#include "core/network/virtual_network_status.hpp"
#include "vpn_engine/native_engine.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"
#include "helper/common/helper_connector.hpp"
#include "platform/common/helper_delegating_network_ops.hpp"
#include "utils/strings.hpp"

#include <algorithm>
#include <cstdio>
#include <cstdint>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <string>
#include <utility>
#include <vector>
#ifndef _WIN32
#include <sys/stat.h>
#endif

namespace ecnuvpn {
namespace app_api {
namespace {
using StageTimer = exv::core::ConnectStageTimer;

config::ConfigManager make_config_manager() {
  platform::ensure_dir(platform::get_config_dir());
  logger::init();
  return config::ConfigManager(platform::get_config_dir());
}

// End inlined from core/app_api/app_api_json_helpers include-unit
// Begin inlined from core/app_api/app_api_controller_helpers include-unit

nlohmann::json preflight_connect(const Config &cfg, const std::string &password) {
  if (cfg.server.empty())
    return error("VPN server is not configured.");
  if (cfg.username.empty())
    return error("VPN username is not configured.");
  if (password.empty())
    return error("VPN password is not configured.");

  if (cfg.vpn_engine == "native") {
    auto native_validation = vpn_engine::validate_native_config(cfg);
    if (!native_validation.ok)
      return error(native_validation.message, native_validation.code);
  }

  platform::BackendResolveOptions options;
  options.preferred_mode = "auto";  // Auto-select: try service first, fallback to oneshot
  options.allow_oneshot = true;     // Allow oneshot mode when service unavailable
  options.start_oneshot = true;     // Start oneshot helper with UAC elevation if needed
  options.allow_service_start = false;
  options.helper_path = helper_binary_next_to_exv();  // Provide helper path for oneshot
  nlohmann::json backend = platform::resolve_backend(options);
  if (!backend.value("ok", false)) {
    return platform::backend_unavailable_error(
        backend, platform::helper_unavailable_connect_message());
  }

  nlohmann::json runtime = runtime_status_json(cfg);
  if (!runtime.value("available", false)) {
    return error("VPN runtime is not available. The desktop bundle is missing the selected VPN engine dependencies.");
  }

  nlohmann::json platform_err =
      platform::preflight_connect_platform_checks(
          config::to_platform_config_view(cfg));
  if (platform_err.is_object() && platform_err.value("ok", true) == false)
    return platform_err;

  // Return success with backend information (including endpoint for oneshot)
  nlohmann::json result;
  result["ok"] = true;
  result["backend"] = backend;
  return result;
}

nlohmann::json logs_json(const nlohmann::json &payload) {
  config::ConfigManager mgr = make_config_manager();
  Config cfg = mgr.load();
  (void)cfg;
  // Read from the unified log path pinned by the runtime module, NOT from
  // cfg.log_file, so the UI sees exactly what every process writes and what
  // `exv logs` shows. Using cfg.log_file here previously diverged from the
  // real log location and made logs appear empty in the app.
  std::string log_path = runtime::paths().log_path;
  int max_lines = payload.value("lines", 100);
  if (max_lines < 1)
    max_lines = 1;
  if (max_lines > 10000)
    max_lines = 10000;
  std::string filter = payload.value("filter", std::string());

  nlohmann::json lines = nlohmann::json::array();
  std::vector<std::string> all_lines;
  std::ifstream ifs(log_path);
  std::string line;
  while (std::getline(ifs, line)) {
    if (!line.empty() && line.back() == '\r')
      line.pop_back();
    if (filter.empty() || line.find(filter) != std::string::npos)
      all_lines.push_back(line);
  }

  size_t start = all_lines.size() > static_cast<size_t>(max_lines)
                     ? all_lines.size() - static_cast<size_t>(max_lines)
                     : 0;
  for (size_t i = start; i < all_lines.size(); ++i) {
    lines.push_back({{"timestamp", ""},
                     {"level", "info"},
                     {"message", json_safe_text(all_lines[i])}});
  }
  return lines;
}
// End inlined from core/app_api/app_api_controller_helpers include-unit
} // namespace
// Begin inlined from core/app_api/app_api_desktop_handlers include-unit
// =========================================================================
// DesktopRpcAdapter — registers all action handlers and delegates
// handle_action() through the AppRpcDispatcher.  This replaces the
// previous giant if/else chain, making app_api.cpp a thin shim.
// =========================================================================

exv::core_api::DesktopRpcAdapter &desktop_adapter() {
  static exv::core_api::DesktopRpcAdapter adapter;
  static bool initialized = false;
  if (!initialized) {
    initialized = true;

    // --- status.get ---
    adapter.register_legacy_handler("status.get",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      Config cfg = mgr.load();
      // Status polling must only query, never initiate a helper connection.
      // ensure_tunnel_controller() is reserved for vpn.connect which has
      // already resolved the backend and started the oneshot helper.
      auto controller = get_tunnel_controller_if_exists();
      if (controller) {
        auto snap = controller->status();
        return frontend_status_from_controller_snapshot(snap, cfg);
      }
      return disconnected_status(cfg);
    });

    // --- vpn.connect ---
    adapter.register_legacy_handler("vpn.connect",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      Config cfg = mgr.load();
      StageTimer timing("desktop.connect");
      std::string password = payload.value("password", std::string());
      logger::info("app_api: vpn.connect entry - password_provided=" +
                   std::string(password.empty() ? "false" : "true") +
                   " server=" + cfg.server + " username=" + cfg.username);
      if (password.empty() && !cfg.password.empty()) {
        std::string key = crypto::load_key();
        if (!key.empty())
          password = crypto::decrypt(cfg.password, key);
      }
      timing.mark("password_resolved",
                  password.empty() ? "source=missing" : "source=available");
      logger::info("app_api: Calling preflight_connect");
      nlohmann::json preflight = preflight_connect(cfg, password);
      if (preflight.is_object() && preflight.value("ok", true) == false) {
        timing.finish(false, "stage=preflight error=" +
                                 json_string(preflight, "error"));
        return preflight;
      }
      timing.mark("preflight", "result=ok");
      if (preflight.is_object() && preflight.contains("backend")) {
        auto backend = preflight["backend"];
        logger::info("app_api: Preflight complete - backend_mode=" + backend.value("mode", "unknown"));
      }
      if (preflight.is_object() && preflight.contains("backend")) {
        auto backend = preflight["backend"];
        logger::info("app_api: Preflight complete - ok=" +
                     std::string(preflight.value("ok", true) ? "true" : "false") +
                     " backend_mode=" + backend.value("mode", "unknown") +
                     " backend_ok=" + std::string(backend.value("ok", false) ? "true" : "false"));
      }

      // Extract helper endpoint from preflight result (for oneshot mode)
      std::string helper_endpoint;
      if (preflight.contains("backend") && preflight["backend"].is_object()) {
        auto backend = preflight["backend"];
        // CRITICAL: Validate backend was actually resolved successfully
        if (!backend.value("ok", false)) {
          timing.finish(false, "stage=backend_resolution error=backend_not_ok");
          return error("Failed to resolve helper backend: " +
                       backend.value("message", std::string("Unknown backend error")),
                       backend.value("code", platform::kHelperUnavailableCode));
        }
        helper_endpoint = backend.value("endpoint", std::string());
        timing.mark("backend_endpoint",
                    helper_endpoint.empty() ? "endpoint=none" : "endpoint=extracted");
      }

      // ── Connection attempt guard ──────────────────────────────────────
      namespace conn_attempt = ecnuvpn::connection_attempt;
      conn_attempt::AcquireOptions attempt_opts;
      attempt_opts.config_dir = platform::get_config_dir();
      attempt_opts.mode = "native";
      attempt_opts.owner_pid = conn_attempt::current_process_id();
      conn_attempt::AcquireResult attempt_result = conn_attempt::try_acquire(attempt_opts);
      timing.mark("connection_attempt",
                  attempt_result.acquired ? "acquired=true" : ("acquired=false code=" + attempt_result.code));

      if (!attempt_result.acquired) {
        nlohmann::json details;
        details["lock_path"] = platform::get_config_dir() + "/connect-attempt.lock";
        details["owner_pid"] = attempt_result.record.owner_pid;
        details["attempt_id"] = attempt_result.record.attempt_id;
        details["created_at_unix_ms"] = attempt_result.record.created_at_unix_ms;
        details["state"] = attempt_result.record.state;
        details["mode"] = attempt_result.record.mode;

        bool owner_alive = false;
        if (attempt_result.record.owner_pid > 0) {
          owner_alive = platform::is_process_alive(attempt_result.record.owner_pid);
        }
        details["owner_alive"] = owner_alive;

        bool stale_detected = !owner_alive && attempt_result.record.owner_pid > 0;
        details["stale_attempt_detected"] = stale_detected;

        if (attempt_result.record.created_at_unix_ms > 0) {
          auto now_ms = std::chrono::duration_cast<std::chrono::milliseconds>(
              std::chrono::system_clock::now().time_since_epoch())
              .count();
          details["owner_age_ms"] = now_ms - attempt_result.record.created_at_unix_ms;
        }

        std::string user_message = attempt_result.message;
        if (stale_detected) {
          user_message = "检测到上次连接尝试异常残留（进程已退出）。正在自动清理...";
          conn_attempt::mark_terminal(platform::get_config_dir(), "stale_auto_cleanup");
          conn_attempt::AcquireResult retry = conn_attempt::try_acquire(attempt_opts);
          if (retry.acquired) {
            timing.mark("connection_attempt_retry", "acquired=true stale_cleanup=true");
            attempt_result = std::move(retry);
          } else {
            timing.finish(false, "stage=connection_attempt retry_failed");
            nlohmann::json resp = error(user_message, attempt_result.code);
            resp["current_attempt"] = details;
            return resp;
          }
        } else {
          timing.finish(false, "stage=connection_attempt");
          nlohmann::json resp = error(user_message, attempt_result.code);
          resp["current_attempt"] = details;
          return resp;
        }
      }

      conn_attempt::TerminalAttemptScope attempt_cleanup(
          platform::get_config_dir(), attempt_result.record.attempt_id, "scope_exit");

      // ── TunnelController connect ──────────────────────────────────────
      // Reset any stale controller state before connecting with new endpoint
      reset_tunnel_controller();
      timing.mark("reset_controller", "stale_state_cleared");

      // Use the helper endpoint from preflight (oneshot or service)
      logger::info("app_api: Initializing TunnelController - endpoint=" +
                   (helper_endpoint.empty() ? "default" : helper_endpoint));
      logger::info("app_api: Initializing TunnelController - endpoint=" +
                   (helper_endpoint.empty() ? "default" : helper_endpoint));
      auto controller = ensure_tunnel_controller(helper_endpoint);
      if (controller) {
        logger::info("app_api: TunnelController initialized successfully");
      }
      timing.mark("tunnel_controller",
                  controller ? "initialized=true" : "initialized=false");

      if (!controller) {
        timing.finish(false, "stage=tunnel_controller_init");
        return error("Failed to initialize VPN controller: " +
                         tunnel_controller_init_error(),
                     platform::kHelperUnavailableCode);
      }

      controller->set_vpn_config(cfg, password);
      timing.mark("cleanup_legacy_state");

      exv::core::UserIntent intent;
      intent.desired_connected = true;
      intent.auto_reconnect = cfg.auto_reconnect;
      intent.profile_id.value = cfg.server;
      logger::info("app_api: Calling TunnelController::connect - server=" + cfg.server);
      logger::info("app_api: Calling TunnelController::connect");
      controller->connect(intent);

      auto snap = controller->status();
      nlohmann::json status =
          frontend_status_from_controller_snapshot(snap, cfg);
      timing.finish(
          true, "phase=" + std::to_string(static_cast<int>(snap.phase)));
      attempt_cleanup.dismiss();
      return status;
    });

    // --- vpn.disconnect ---
    adapter.register_legacy_handler("vpn.disconnect",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      Config cfg = mgr.load();
      auto controller = get_tunnel_controller_if_exists();
      if (controller) {
        controller->disconnect(exv::core::DisconnectReason::UserRequested);
      }
      return disconnected_status(cfg);
    });

    // --- config.getAuth ---
    adapter.register_legacy_handler("config.getAuth",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      Config cfg = mgr.load();
      return auth_config(cfg);
    });

    // --- config.saveAuth ---
    adapter.register_legacy_handler("config.saveAuth",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      if (payload.contains("server") && payload["server"].is_string()) {
        std::string err = config_api::config_set(mgr, "server", payload["server"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      if (payload.contains("username") && payload["username"].is_string()) {
        std::string err = config_api::config_set(mgr, "username", payload["username"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      bool remember_payload = payload.contains("remember_password") &&
                              payload["remember_password"].is_boolean();
      bool remember_password =
          remember_payload ? payload["remember_password"].get<bool>()
                           : mgr.load().remember_password;
      std::string submitted_password =
          payload.contains("password") && payload["password"].is_string()
              ? payload["password"].get<std::string>()
              : std::string();

      if (remember_payload && !remember_password) {
        std::string err = config_api::config_clear_password_and_key(mgr);
        if (!err.empty()) return error(err);
      } else if (remember_password) {
        if (!submitted_password.empty()) {
          std::string err = config_api::config_set_password(mgr, submitted_password);
          if (!err.empty()) return error(err);
        } else {
          Config current = mgr.load();
          if (remember_payload && current.password.empty()) {
            return error("Password is required to enable remember_password.");
          }
          std::string err = config_api::config_set(mgr, "remember_password", "true");
          if (!err.empty()) return error(err);
        }
      }
      if (payload.contains("user_agent") && payload["user_agent"].is_string()) {
        std::string value = payload["user_agent"].get<std::string>();
        if (!exv::utils::trim(value).empty()) {
          std::string err = config_api::config_set(mgr, "useragent", value);
          if (!err.empty()) return error(err);
        }
      }
      return auth_config(mgr.load());
    });

    // --- config.getSettings ---
    adapter.register_legacy_handler("config.getSettings",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      Config cfg = mgr.load();
      return settings_config(cfg);
    });

    // --- config.saveSettings ---
    adapter.register_legacy_handler("config.saveSettings",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      if (payload.contains("mtu") && payload["mtu"].is_number_integer()) {
        std::string err = config_api::config_set(mgr, "mtu", std::to_string(payload["mtu"].get<int>()));
        if (!err.empty()) return error(err);
      }
      if (payload.contains("dtls") && payload["dtls"].is_boolean()) {
        std::string err = config_api::config_set(mgr, "disable_dtls",
                               payload["dtls"].get<bool>() ? "false" : "true");
        if (!err.empty()) return error(err);
      }
      if (payload.contains("extra_args") && payload["extra_args"].is_string()) {
        Config updated = mgr.load();
        std::string value = payload["extra_args"].get<std::string>();
        updated.extra_args = value.empty() ? std::vector<std::string>{}
                                           : std::vector<std::string>{value};
        mgr.save(updated);
      }
      if (payload.contains("log_path") && payload["log_path"].is_string()) {
        std::string err = config_api::config_set(mgr, "log_file", payload["log_path"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      if (payload.contains("vpn_engine") && payload["vpn_engine"].is_string()) {
        std::string err =
            config_api::config_set(mgr, "vpn_engine",
                                   payload["vpn_engine"].get<std::string>());
        if (!err.empty())
          return error(err);
      }
      if (payload.contains("auto_reconnect") &&
          payload["auto_reconnect"].is_boolean()) {
        std::string err = config_api::config_set(
            mgr, "auto_reconnect",
            payload["auto_reconnect"].get<bool>() ? "true" : "false");
        if (!err.empty()) return error(err);
      }
      if (payload.contains("minimal_mode") &&
          payload["minimal_mode"].is_boolean()) {
        std::string err = config_api::config_set(
            mgr, "minimal_mode",
            payload["minimal_mode"].get<bool>() ? "true" : "false");
        if (!err.empty()) return error(err);
      }
      if (payload.contains("service_install_prompt_seen") &&
          payload["service_install_prompt_seen"].is_boolean()) {
        std::string err = config_api::config_set(
            mgr, "service_install_prompt_seen",
            payload["service_install_prompt_seen"].get<bool>() ? "true"
                                                               : "false");
        if (!err.empty()) return error(err);
      }
      if (payload.contains("minimal_install_service_before_connect") &&
          payload["minimal_install_service_before_connect"].is_boolean()) {
        std::string err = config_api::config_set(
            mgr, "minimal_install_service_before_connect",
            payload["minimal_install_service_before_connect"].get<bool>()
                ? "true"
                : "false");
        if (!err.empty()) return error(err);
      }
      if (payload.contains("openconnect_runtime") &&
          payload["openconnect_runtime"].is_string()) {
        std::string err = config_api::config_set(mgr, "openconnect_runtime",
                               payload["openconnect_runtime"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      if (payload.contains("windows_tunnel_driver") &&
          payload["windows_tunnel_driver"].is_string()) {
        std::string err = config_api::config_set(mgr, "windows_tunnel_driver",
                               payload["windows_tunnel_driver"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      if (payload.contains("windows_tap_interface") &&
          payload["windows_tap_interface"].is_string()) {
        std::string err = config_api::config_set(mgr, "windows_tap_interface",
                               payload["windows_tap_interface"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      return settings_config(mgr.load());
    });

    // --- config.getKey ---
    adapter.register_legacy_handler("config.getKey",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      return key_status_json();
    });

    // --- routes.list ---
    adapter.register_legacy_handler("routes.list",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      Config cfg = mgr.load();
      return routes_json(cfg);
    });

    // --- routes.add ---
    adapter.register_legacy_handler("routes.add",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      std::string err = config_api::route_add(mgr, payload.value("cidr", ""));
      if (!err.empty())
        return error(err);
      return routes_json(mgr.load());
    });

    // --- routes.remove ---
    adapter.register_legacy_handler("routes.remove",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      std::string err = config_api::route_remove(mgr, payload.value("cidr", ""));
      if (!err.empty())
        return error(err);
      return routes_json(mgr.load());
    });

    // --- routes.reset ---
    adapter.register_legacy_handler("routes.reset",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      config_api::route_reset_defaults(mgr);
      return routes_json(mgr.load());
    });

    // --- service.status ---
    adapter.register_legacy_handler("service.status",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      return service_status_json();
    });

    // --- helper.status ---
    adapter.register_legacy_handler("helper.status",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      platform::BackendResolveOptions options;
      options.preferred_mode = "auto";
      options.allow_oneshot = true;
      options.allow_service_start = false;
      nlohmann::json resolved = platform::resolve_backend(options);
      if (!resolved.value("ok", false)) {
        resolved["resolved"] = false;
        resolved["resolution_code"] = resolved.value("code", std::string());
        resolved["resolution_message"] =
            resolved.value("message", std::string());
        resolved["ok"] = true;
      } else {
        resolved["resolved"] = true;
      }
      return resolved;
    });

    // --- runtime.status ---
    adapter.register_legacy_handler("runtime.status",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      Config cfg = mgr.load();
      return runtime_status_json(cfg);
    });

    // --- drivers.status ---
    adapter.register_legacy_handler("drivers.status",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      Config cfg = mgr.load();
      return driver_status_json(cfg);
    });

    // --- drivers.install ---
    adapter.register_legacy_handler("drivers.install",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      config::ConfigManager mgr = make_config_manager();
      Config cfg = mgr.load();
      return install_driver(cfg, payload);
    });

    // --- logs.list ---
    adapter.register_legacy_handler("logs.list",
        [](const nlohmann::json &payload) -> nlohmann::json {
      apply_desktop_runtime_context(payload);
      return logs_json(payload);
    });
  }
  return adapter;
}

nlohmann::json dispatch_desktop_action(const std::string &action,
                                       const nlohmann::json &payload) {
  try {
    return desktop_adapter().dispatch(action, payload);
  } catch (const std::exception &ex) {
    return error(ex.what());
  } catch (...) {
    return error("Unknown desktop API error");
  }
}

// End inlined from core/app_api/app_api_desktop_handlers include-unit
} // namespace app_api
} // namespace ecnuvpn
