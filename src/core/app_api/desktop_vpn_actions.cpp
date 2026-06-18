#include "core/app_api/desktop_vpn_actions.hpp"

#include "core/app_api/desktop_json.hpp"
#include "core/app_api/desktop_runtime_context.hpp"
#include "core/app_api/desktop_status_presenter.hpp"
#include "core/app_api/desktop_tunnel_host.hpp"
#include "core/config/config_manager.hpp"
#include "core/config/config_platform_view.hpp"
#include "core/connection/connection_attempt.hpp"
#include "core/crypto/crypto.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "core/tunnel_controller/native_engine_config_mapper.hpp"
#include "core/tunnel_controller/timing.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"
#include "observability/log_facade.hpp"
#include "platform/common/logging/log_runtime.hpp"
#include "platform/common/app_api_runtime_policy.hpp"
#include "platform/common/backend_resolver.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/helper_client.hpp"
#include "platform/common/process_control.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_paths.hpp"

#include <chrono>
#include <string>

namespace ecnuvpn {
namespace app_api {
namespace {

using StageTimer = exv::core::ConnectStageTimer;

config::ConfigManager make_config_manager() {
  platform::ensure_dir(platform::get_config_dir());
  ecnuvpn::platform::logging::configure_default_logging(false);
  return config::ConfigManager(platform::get_config_dir());
}

nlohmann::json preflight_connect(const Config &cfg,
                                 const std::string &password) {
  if (cfg.vpn_engine != "native") {
    return error("VPN engine is native-only.", "legacy_engine_removed");
  }
  if (cfg.server.empty()) {
    return error("VPN server is not configured.");
  }
  if (cfg.username.empty()) {
    return error("VPN username is not configured.");
  }
  if (password.empty()) {
    return error("VPN password is not configured.");
  }

  auto native_validation = exv::core::validate_native_app_config(cfg);
  if (!native_validation.ok) {
    return error(native_validation.message, native_validation.code);
  }

  platform::BackendResolveOptions options;
  options.preferred_mode = "auto";
  options.allow_oneshot = true;
  options.start_oneshot = true;
  options.allow_service_start = false;
  options.helper_path = helper_binary_next_to_exv();
  nlohmann::json backend = platform::resolve_backend(options);
  if (!backend.value("ok", false)) {
    return platform::backend_unavailable_error(
        backend, platform::helper_unavailable_connect_message());
  }

  nlohmann::json runtime = runtime_status_json(cfg);
  if (!runtime.value("available", false)) {
    return error("VPN runtime is not available. The desktop bundle is missing "
                 "the selected VPN engine dependencies.");
  }

  nlohmann::json platform_err =
      platform::preflight_connect_platform_checks(
          config::to_platform_config_view(cfg));
  if (platform_err.is_object() && platform_err.value("ok", true) == false) {
    return platform_err;
  }

  nlohmann::json result;
  result["ok"] = true;
  result["backend"] = backend;
  return result;
}

nlohmann::json auth_interaction_json(
    const exv::core::TunnelController::PendingAuthInteraction &pending) {
  return nlohmann::json{{"id", pending.id},
                        {"kind", pending.kind},
                        {"label", pending.label},
                        {"input_type", pending.input_type},
                        {"options", pending.options}};
}

} // namespace

void register_desktop_vpn_actions(exv::core_api::DesktopRpcAdapter &adapter) {
  adapter.register_legacy_handler(
      "status.get", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        Config cfg = mgr.load();
        auto controller = get_tunnel_controller_if_exists();
        if (controller) {
          auto snap = controller->status();
          return frontend_status_from_controller_snapshot(snap, cfg);
        }
        return disconnected_status(cfg);
      });

  adapter.register_legacy_handler(
      "vpn.connect", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        Config cfg = mgr.load();
        StageTimer timing("desktop.connect");
        std::string password = payload.value("password", std::string());
        exv::observability::LogFacade::info("app_api: vpn.connect entry - password_provided=" +
                     std::string(password.empty() ? "false" : "true") +
                     " server=" + cfg.server + " username=" + cfg.username);
        if (password.empty() && !cfg.password.empty()) {
          std::string key = crypto::load_key();
          if (!key.empty()) {
            password = crypto::decrypt(cfg.password, key);
          }
        }
        timing.mark("password_resolved",
                    password.empty() ? "source=missing" : "source=available");
        exv::observability::LogFacade::info("app_api: Calling preflight_connect");
        nlohmann::json preflight = preflight_connect(cfg, password);
        if (preflight.is_object() && preflight.value("ok", true) == false) {
          timing.finish(false, "stage=preflight error=" +
                                   json_string(preflight, "error"));
          return preflight;
        }
        timing.mark("preflight", "result=ok");
        if (preflight.is_object() && preflight.contains("backend")) {
          auto backend = preflight["backend"];
          exv::observability::LogFacade::info("app_api: Preflight complete - backend_mode=" +
                       backend.value("mode", "unknown"));
        }
        if (preflight.is_object() && preflight.contains("backend")) {
          auto backend = preflight["backend"];
          exv::observability::LogFacade::info("app_api: Preflight complete - ok=" +
                       std::string(preflight.value("ok", true) ? "true"
                                                               : "false") +
                       " backend_mode=" + backend.value("mode", "unknown") +
                       " backend_ok=" +
                       std::string(backend.value("ok", false) ? "true"
                                                               : "false"));
        }

        std::string helper_endpoint;
        if (preflight.contains("backend") && preflight["backend"].is_object()) {
          auto backend = preflight["backend"];
          if (!backend.value("ok", false)) {
            timing.finish(false,
                          "stage=backend_resolution error=backend_not_ok");
            return error("Failed to resolve helper backend: " +
                             backend.value(
                                 "message",
                                 std::string("Unknown backend error")),
                         backend.value("code",
                                       platform::kHelperUnavailableCode));
          }
          helper_endpoint = backend.value("endpoint", std::string());
          timing.mark("backend_endpoint",
                      helper_endpoint.empty() ? "endpoint=none"
                                              : "endpoint=extracted");
        }

        namespace conn_attempt = ecnuvpn::connection_attempt;
        conn_attempt::AcquireOptions attempt_opts;
        attempt_opts.config_dir = platform::get_config_dir();
        attempt_opts.mode = "native";
        attempt_opts.owner_pid = conn_attempt::current_process_id();
        conn_attempt::AcquireResult attempt_result =
            conn_attempt::try_acquire(attempt_opts);
        timing.mark("connection_attempt",
                    attempt_result.acquired
                        ? "acquired=true"
                        : ("acquired=false code=" + attempt_result.code));

        if (!attempt_result.acquired) {
          nlohmann::json details;
          details["lock_path"] =
              platform::get_config_dir() + "/connect-attempt.lock";
          details["owner_pid"] = attempt_result.record.owner_pid;
          details["attempt_id"] = attempt_result.record.attempt_id;
          details["created_at_unix_ms"] =
              attempt_result.record.created_at_unix_ms;
          details["state"] = attempt_result.record.state;
          details["mode"] = attempt_result.record.mode;

          bool owner_alive = false;
          if (attempt_result.record.owner_pid > 0) {
            owner_alive =
                platform::is_process_alive(attempt_result.record.owner_pid);
          }
          details["owner_alive"] = owner_alive;

          const bool stale_detected =
              !owner_alive && attempt_result.record.owner_pid > 0;
          details["stale_attempt_detected"] = stale_detected;

          if (attempt_result.record.created_at_unix_ms > 0) {
            auto now_ms = std::chrono::duration_cast<std::chrono::milliseconds>(
                              std::chrono::system_clock::now()
                                  .time_since_epoch())
                              .count();
            details["owner_age_ms"] =
                now_ms - attempt_result.record.created_at_unix_ms;
          }

          std::string user_message = attempt_result.message;
          if (stale_detected) {
            user_message =
                "检测到上次连接尝试异常残留（进程已退出）。正在自动清理...";
            conn_attempt::mark_terminal(platform::get_config_dir(),
                                        "stale_auto_cleanup");
            conn_attempt::AcquireResult retry =
                conn_attempt::try_acquire(attempt_opts);
            if (retry.acquired) {
              timing.mark("connection_attempt_retry",
                          "acquired=true stale_cleanup=true");
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
            platform::get_config_dir(), attempt_result.record.attempt_id,
            "scope_exit");

        reset_tunnel_controller();
        timing.mark("reset_controller", "stale_state_cleared");

        exv::observability::LogFacade::info("app_api: Initializing TunnelController - endpoint=" +
                     (helper_endpoint.empty() ? "default" : helper_endpoint));
        exv::observability::LogFacade::info("app_api: Initializing TunnelController - endpoint=" +
                     (helper_endpoint.empty() ? "default" : helper_endpoint));
        auto controller = ensure_tunnel_controller(helper_endpoint);
        if (controller) {
          exv::observability::LogFacade::info("app_api: TunnelController initialized successfully");
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
        exv::observability::LogFacade::info("app_api: Calling TunnelController::connect - server=" +
                     cfg.server);
        exv::observability::LogFacade::info("app_api: Calling TunnelController::connect");
        controller->connect(intent);

        auto snap = controller->status();
        nlohmann::json status =
            frontend_status_from_controller_snapshot(snap, cfg);
        timing.finish(
            true, "phase=" + std::to_string(static_cast<int>(snap.phase)));
        attempt_cleanup.dismiss();
        return status;
      });

  adapter.register_legacy_handler(
      "vpn.authInteraction.get",
      [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        auto controller = get_tunnel_controller_if_exists();
        if (!controller) {
          return nlohmann::json{{"ok", true}, {"pending", false}};
        }
        auto pending = controller->pending_auth_interaction();
        if (!pending) {
          return nlohmann::json{{"ok", true}, {"pending", false}};
        }
        return nlohmann::json{{"ok", true},
                              {"pending", true},
                              {"interaction", auth_interaction_json(*pending)}};
      });

  adapter.register_legacy_handler(
      "vpn.authInteraction.respond",
      [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        auto controller = get_tunnel_controller_if_exists();
        if (!controller) {
          return error("No active VPN controller.", "invalid_request");
        }
        const std::string id = payload.value("id", std::string());
        const std::string value = payload.value("value", std::string());
        if (id.empty()) {
          return error("Missing auth interaction id.", "invalid_request");
        }
        if (!controller->provide_auth_interaction_response(id, value)) {
          return error("Auth interaction is no longer pending.",
                       "invalid_request");
        }
        return nlohmann::json{{"ok", true}};
      });

  adapter.register_legacy_handler(
      "vpn.disconnect", [](const nlohmann::json &payload) -> nlohmann::json {
        apply_desktop_runtime_context(payload);
        config::ConfigManager mgr = make_config_manager();
        Config cfg = mgr.load();
        auto controller = get_tunnel_controller_if_exists();
        if (controller) {
          controller->disconnect(exv::core::DisconnectReason::UserRequested);
        }
        return disconnected_status(cfg);
      });
}

} // namespace app_api
} // namespace ecnuvpn
