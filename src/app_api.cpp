#include "app_api.hpp"

#include "config.hpp"
#include "config_api.hpp"
#include "config_manager.hpp"
#include "connection_attempt.hpp"
#include "core/timing.hpp"
#include "core_api/desktop_rpc_adapter.hpp"
#include "crypto.hpp"
#include "feedback/feedback.hpp"
#include "helper.hpp"
#include "logger.hpp"
#include "platform/common/app_api_runtime_policy.hpp"
#include "platform/common/backend_resolver.hpp"
#include "platform/common/helper_client.hpp"
#include "platform/common/driver_status.hpp"
#include "platform/common/oneshot_bootstrap.hpp"
#include "platform/common/process_control.hpp"
#include "platform/common/runtime_status.hpp"
#include "platform/common/service_status.hpp"
#include "runtime/runtime_context.hpp"
#include "core/tunnel_controller_active.hpp"
#include "utils.hpp"
#include "virtual_network.hpp"
#include "vpn_engine/native_engine.hpp"
#include "vpn_engine/native_session_store.hpp"
#include "core/tunnel_controller.hpp"
#include "helper_common/helper_connector.hpp"
#include "platform/common/helper_delegating_network_ops.hpp"

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

nlohmann::json error(const std::string &message,
                     const std::string &code = std::string()) {
  // Route through the unified feedback module: guarantees a canonical,
  // non-empty code plus recoverable/recommended_action. The frontend contract
  // uses the "error" key for the human message.
  feedback::ErrorInfo info = feedback::lookup_error(code, message);
  return nlohmann::json{{"ok", false},
                        {"error", message},
                        {"code", info.code},
                        {"recoverable", info.recoverable},
                        {"recommended_action", info.recommended_action}};
}

std::string json_string(const nlohmann::json &object, const char *key,
                        const std::string &fallback = std::string());

bool helper_unavailable(const nlohmann::json &response) {
  const std::string code = json_string(response, "code");
  return code == platform::kHelperUnavailableCode ||
         code == platform::kServiceNotInstalledCode ||
         code == platform::kServiceInstalledNotRunningCode ||
         json_string(response, "message") == "Helper daemon not available";
}

using StageTimer = exv::core::ConnectStageTimer;

bool json_bool(const nlohmann::json &object, const char *key, bool fallback) {
  if (!object.is_object() || !object.contains(key) || object[key].is_null())
    return fallback;
  if (object[key].is_boolean())
    return object[key].get<bool>();
  return fallback;
}

int json_int(const nlohmann::json &object, const char *key, int fallback) {
  if (!object.is_object() || !object.contains(key) || object[key].is_null())
    return fallback;
  if (object[key].is_number_integer())
    return object[key].get<int>();
  return fallback;
}

uint64_t json_u64(const nlohmann::json &object, const char *key,
                  uint64_t fallback) {
  if (!object.is_object() || !object.contains(key) || object[key].is_null())
    return fallback;
  if (object[key].is_number_unsigned())
    return object[key].get<uint64_t>();
  if (object[key].is_number_integer()) {
    int64_t value = object[key].get<int64_t>();
    return value < 0 ? fallback : static_cast<uint64_t>(value);
  }
  return fallback;
}

std::string json_string(const nlohmann::json &object, const char *key,
                        const std::string &fallback) {
  if (!object.is_object() || !object.contains(key) || object[key].is_null())
    return fallback;
  if (object[key].is_string())
    return object[key].get<std::string>();
  return fallback;
}

nlohmann::json helper_error(const nlohmann::json &response,
                            const std::string &fallback_message) {
  return error(json_string(response, "message", fallback_message),
               json_string(response, "code"));
}

config::ConfigManager make_config_manager() {
  utils::ensure_dir(utils::get_config_dir());
  logger::init();
  return config::ConfigManager(utils::get_config_dir());
}

void apply_desktop_runtime_context(const nlohmann::json &payload) {
  if (!payload.is_object())
    return;

  std::string home = json_string(payload, "home");
  std::string config_dir = json_string(payload, "config_dir");
  if (home.empty() && config_dir.empty())
    return;

  utils::set_runtime_path_override(home.empty() ? utils::get_effective_home()
                                                : home,
                                   config_dir);
#ifndef _WIN32
  std::string owner_home = home.empty() ? utils::get_effective_home() : home;
  struct stat home_stat {};
  if (!owner_home.empty() && stat(owner_home.c_str(), &home_stat) == 0) {
    utils::set_runtime_owner(home_stat.st_uid, home_stat.st_gid);
  }
#endif
}

void add_desktop_owner_context(nlohmann::json &request) {
#ifndef _WIN32
  if (!utils::has_runtime_owner())
    return;
  request["owner_uid"] =
      static_cast<unsigned int>(utils::get_runtime_owner_uid());
  request["owner_gid"] =
      static_cast<unsigned int>(utils::get_runtime_owner_gid());
#else
  (void)request;
#endif
}

nlohmann::json frontend_status_from_helper(const nlohmann::json &helper_resp,
                                           const Config &cfg) {
  bool running = json_bool(helper_resp, "running", false);
  bool network_ready = json_bool(helper_resp, "network_ready", false);
  nlohmann::json j;
  j["connected"] = running && network_ready;
  j["process_running"] = running;
  j["server"] = json_string(helper_resp, "server", cfg.server);
  j["username"] = cfg.username;
  j["pid"] = json_int(helper_resp, "pid", -1);
  j["supervisor_pid"] = json_int(helper_resp, "supervisor_pid", -1);
  j["network_ready"] = network_ready;
  j["interface"] = json_string(helper_resp, "interface");
  j["internal_ip"] = json_string(helper_resp, "internal_ip");
  j["route_count"] =
      json_int(helper_resp, "route_count", static_cast<int>(cfg.routes.size()));
  j["mtu"] = cfg.mtu;
  j["uptime_seconds"] = 0;
  j["rx_bytes"] = json_u64(helper_resp, "rx_bytes", 0);
  j["tx_bytes"] = json_u64(helper_resp, "tx_bytes", 0);
  try {
    virtual_network::add_status_fields(j, json_string(j, "interface"));
  } catch (...) {
  }
  return j;
}

nlohmann::json disconnected_status(const Config &cfg) {
  nlohmann::json j{{"connected", false},
                   {"process_running", false},
                   {"server", cfg.server},
                   {"username", cfg.username},
                   {"pid", -1},
                   {"supervisor_pid", -1},
                   {"network_ready", false},
                   {"interface", ""},
                   {"internal_ip", ""},
                   {"route_count", static_cast<int>(cfg.routes.size())},
                   {"mtu", cfg.mtu},
                   {"uptime_seconds", 0},
                   {"rx_bytes", 0},
                   {"tx_bytes", 0},
                   {"upstream_virtual_detected", false},
                   {"upstream_virtual_adapters", nlohmann::json::array()},
                   {"upstream_virtual_message", ""},
                   {"route_policy", "normal"}};
  try {
    virtual_network::add_status_fields(j, "");
  } catch (...) {
  }
  return j;
}

nlohmann::json frontend_status_from_snapshot_json(const nlohmann::json &snapshot,
                                                   const Config &cfg) {
  std::string iface = json_string(snapshot, "interface");
  bool running = json_bool(snapshot, "running", false);
  bool network_ready = json_bool(snapshot, "network_ready", false);
  uint64_t rx_bytes = 0;
  uint64_t tx_bytes = 0;
  if (network_ready && !iface.empty()) {
    utils::get_interface_traffic(iface, &rx_bytes, &tx_bytes);
  }

  nlohmann::json j;
  j["connected"] = running && network_ready;
  j["process_running"] = running;
  j["server"] = cfg.server;
  j["username"] = cfg.username;
  j["pid"] = json_int(snapshot, "pid", -1);
  j["supervisor_pid"] = json_int(snapshot, "supervisor_pid", -1);
  j["network_ready"] = network_ready;
  j["interface"] = iface;
  j["internal_ip"] = json_string(snapshot, "internal_ip");
  j["route_count"] = static_cast<int>(cfg.routes.size());
  j["mtu"] = cfg.mtu;
  j["uptime_seconds"] = 0;
  j["rx_bytes"] = rx_bytes;
  j["tx_bytes"] = tx_bytes;
  try {
    virtual_network::add_status_fields(j, iface);
  } catch (...) {
  }
  return j;
}

nlohmann::json auth_config(const Config &cfg) {
  // Never echo the stored ciphertext or a fake mask back to the UI. The UI
  // shows a placeholder when password_stored is true and treats an empty
  // submitted password as "keep the existing one".
  return nlohmann::json{{"server", cfg.server},
                        {"username", cfg.username},
                        {"password", ""},
                        {"password_stored", !cfg.password.empty()},
                        {"user_agent", cfg.useragent},
                        {"remember_password", cfg.remember_password}};
}

nlohmann::json settings_config(const Config &cfg) {
  std::string extra_args;
  for (size_t i = 0; i < cfg.extra_args.size(); ++i) {
    if (i > 0)
      extra_args += " ";
    extra_args += cfg.extra_args[i];
  }

  return nlohmann::json{{"mtu", cfg.mtu},
                        {"dtls", !cfg.disable_dtls},
                        {"extra_args", extra_args},
                        {"log_path", cfg.log_file},
                        {"webui_port", cfg.webui_port},
                        {"webui_host", cfg.webui_bind},
                        {"webui_enabled", cfg.webui_enabled},
                        {"vpn_engine", cfg.vpn_engine},
                        {"openconnect_runtime", cfg.openconnect_runtime},
                        {"windows_tunnel_driver", cfg.windows_tunnel_driver},
                        {"windows_tap_interface", cfg.windows_tap_interface},
                        {"auto_reconnect", cfg.auto_reconnect},
                        {"minimal_mode", cfg.minimal_mode},
                        {"service_install_prompt_seen",
                         cfg.service_install_prompt_seen},
                        {"minimal_install_service_before_connect",
                         cfg.minimal_install_service_before_connect}};
}

nlohmann::json routes_json(const Config &cfg) {
  nlohmann::json arr = nlohmann::json::array();
  for (const auto &route : cfg.routes) {
    arr.push_back({{"cidr", route}});
  }
  return arr;
}

nlohmann::json key_status_json() {
  std::string status = config_api::key_status();
  return nlohmann::json{{"present", status == "valid"},
                        {"fingerprint", status == "valid"
                                            ? nlohmann::json("active")
                                            : nlohmann::json(nullptr)},
                        {"status", status}};
}

nlohmann::json service_status_json() {
  return platform::service_status_to_json(platform::current_service_status());
}

std::string helper_binary_next_to_exv() {
  std::filesystem::path exv_path(utils::get_executable_path());
#ifdef _WIN32
  return (exv_path.parent_path() / "exv-helper.exe").string();
#else
  return (exv_path.parent_path() / "exv-helper").string();
#endif
}

std::string json_safe_text(const std::string &text) {
  std::string out;
  out.reserve(text.size());
  for (unsigned char c : text) {
    if (c == '\t' || c == '\n' || c == '\r' || (c >= 0x20 && c < 0x80)) {
      out.push_back(static_cast<char>(c));
    } else {
      out.push_back('?');
    }
  }
  return out;
}

nlohmann::json runtime_status_json(const Config &cfg) {
  return platform::runtime_status_json(cfg);
}

// =========================================================================
// D3: Legacy state-file cleanup for TunnelController path.
//
// When connecting via TunnelController (Core-owned mode), the in-memory
// TunnelStatusSnapshot replaces file-based persistence.  Any leftover
// native-session-state.json or ecnuvpn-supervisor.pid from a previous
// legacy-supervisor session must be cleaned up so crash recovery does not
// misinterpret stale state.  The read-side functions (load_native_session_state,
// read_native_session_snapshot) are preserved for crash recovery but the
// new architecture never writes these files.
// =========================================================================

// =========================================================================
// TunnelController singleton — lazily initialized on first VPN action.
// Holds the HelperClient, PlatformNetworkOps, and TunnelController as a
// single unit so their lifetimes are correctly managed.
// =========================================================================

struct TunnelControllerHolder {
  std::unique_ptr<exv::helper::HelperConnector> connector;
  std::shared_ptr<exv::helper::HelperClient> client;
  std::shared_ptr<exv::platform::HelperDelegatingPlatformNetworkOps> net_ops;
  std::shared_ptr<exv::core::TunnelController> controller;
  bool init_attempted = false;
  std::string init_error;
};

TunnelControllerHolder &tunnel_holder() {
  static TunnelControllerHolder holder;
  return holder;
}

/// Lazily create and connect the TunnelController. Returns nullptr if the
/// helper daemon cannot be reached. Subsequent calls after a failure will
/// retry (the helper may have been installed or started in the meantime).
std::shared_ptr<exv::core::TunnelController> ensure_tunnel_controller() {
  auto &h = tunnel_holder();
  if (h.controller) {
    exv::core::set_tunnel_controller_active(true);
    return h.controller;
  }

  h.init_attempted = true;
  try {
    h.connector = exv::helper::HelperConnector::create();
    exv::helper::HelperConnectorConfig cc;
    cc.mode = exv::helper::ConnectorMode::Transient;
    cc.helper_executable_path = helper_binary_next_to_exv();
    h.client = h.connector->connect(cc);
    if (!h.client) {
      h.init_error = "Failed to connect to helper daemon";
      return nullptr;
    }
    h.net_ops =
        std::make_shared<exv::platform::HelperDelegatingPlatformNetworkOps>(
            h.client.get());
    h.controller = std::make_shared<exv::core::TunnelController>(h.client,
                                                                  h.net_ops);
    exv::core::set_tunnel_controller_active(true);
    return h.controller;
  } catch (const std::exception &e) {
    h.init_error = e.what();
    return nullptr;
  }
}

/// Return the existing TunnelController if already initialized, or nullptr
/// without attempting to create one.
std::shared_ptr<exv::core::TunnelController> get_tunnel_controller_if_exists() {
  return tunnel_holder().controller;
}

/// Map a TunnelStatusSnapshot to the frontend-compatible JSON format that
/// the WebUI expects. Field names and types match the legacy helper response.
nlohmann::json frontend_status_from_controller_snapshot(
    const exv::core::TunnelStatusSnapshot &snap, const Config &cfg) {
  bool running = snap.phase != exv::core::TunnelPhase::Idle &&
                 snap.phase != exv::core::TunnelPhase::Failed;
  bool connected = running && snap.network_ready;

  nlohmann::json j;
  j["connected"] = connected;
  j["process_running"] = running;
  j["server"] = snap.server.empty() ? cfg.server : snap.server;
  j["username"] = cfg.username;
  j["pid"] = -1;           // TunnelController does not expose raw PIDs
  j["supervisor_pid"] = -1;
  j["network_ready"] = snap.network_ready;
  j["interface"] = snap.interface_name;
  j["internal_ip"] = "";
  j["route_count"] = static_cast<int>(cfg.routes.size());
  j["mtu"] = cfg.mtu;
  j["uptime_seconds"] = 0;
  j["rx_bytes"] = 0;
  j["tx_bytes"] = 0;
  if (snap.last_error.has_value()) {
    j["error"] = snap.last_error->message;
    j["error_code"] = snap.last_error->code;
    j["error_recoverable"] = snap.last_error->recoverable;
  }
  if (snap.reconnect.has_value()) {
    j["reconnect_attempt"] = snap.reconnect->attempt;
    j["reconnect_next_retry_ms"] = snap.reconnect->next_retry_ms;
  }
  j["auto_reconnect"] = snap.auto_reconnect;
  j["phase"] = [snap]() -> std::string {
    switch (snap.phase) {
    case exv::core::TunnelPhase::Idle: return "idle";
    case exv::core::TunnelPhase::PreparingHelper: return "preparing_helper";
    case exv::core::TunnelPhase::Authenticating: return "authenticating";
    case exv::core::TunnelPhase::ConnectingCstp: return "connecting_cstp";
    case exv::core::TunnelPhase::ApplyingNetworkConfig:
      return "applying_network_config";
    case exv::core::TunnelPhase::OpeningPacketDevice:
      return "opening_packet_device";
    case exv::core::TunnelPhase::Connected: return "connected";
    case exv::core::TunnelPhase::Reconnecting: return "reconnecting";
    case exv::core::TunnelPhase::Disconnecting: return "disconnecting";
    case exv::core::TunnelPhase::CleaningUp: return "cleaning_up";
    case exv::core::TunnelPhase::Failed: return "failed";
    default: return "unknown";
    }
  }();
  try {
    virtual_network::add_status_fields(j, snap.interface_name);
  } catch (...) {
  }
  return j;
}

nlohmann::json driver_status_json(const Config &cfg) {
  return platform::driver_status_json(cfg);
}

nlohmann::json install_driver(const Config &cfg, const nlohmann::json &payload) {
  return platform::install_driver(cfg, payload);
}

nlohmann::json preflight_connect(const Config &cfg, const std::string &password,
                                bool allow_direct_fallback = false) {
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
  options.preferred_mode = "service";
  options.allow_oneshot = false;
  options.allow_service_start = false;
  nlohmann::json backend = platform::resolve_backend(options);
  if (!backend.value("ok", false)) {
    nlohmann::json unavailable = platform::backend_unavailable_error(
        backend, platform::helper_unavailable_connect_message());
    if (!allow_direct_fallback || !helper_unavailable(unavailable)) {
      return unavailable;
    }
  }

  nlohmann::json runtime = runtime_status_json(cfg);
  if (!runtime.value("available", false)) {
    return error("VPN runtime is not available. The desktop bundle is missing the selected VPN engine dependencies.");
  }

  nlohmann::json platform_err = platform::preflight_connect_platform_checks(cfg);
  if (platform_err.is_object() && platform_err.value("ok", true) == false)
    return platform_err;

  return nlohmann::json{{"ok", true}};
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

} // namespace

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
      auto controller = get_tunnel_controller_if_exists();
      if (!controller) {
        controller = ensure_tunnel_controller();
      }
      if (controller) {
        auto snap = controller->status();
        return frontend_status_from_controller_snapshot(snap, cfg);
      }
      nlohmann::json fallback = platform::status_fallback_without_helper(cfg);
      if (fallback.is_object() && fallback.value("_snapshot", false)) {
        return frontend_status_from_snapshot_json(
            fallback.value("_snapshot_data", nlohmann::json::object()), cfg);
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
      if (password.empty() && !cfg.password.empty()) {
        std::string key = crypto::load_key();
        if (!key.empty())
          password = crypto::decrypt(cfg.password, key);
      }
      timing.mark("password_resolved",
                  password.empty() ? "source=missing" : "source=available");
      const bool allow_direct_fallback =
          payload.value("allow_direct_fallback", false);
      nlohmann::json preflight =
          preflight_connect(cfg, password, allow_direct_fallback);
      if (preflight.is_object() && preflight.value("ok", true) == false) {
        timing.finish(false, "stage=preflight error=" +
                                 json_string(preflight, "error"));
        return preflight;
      }
      timing.mark("preflight", "result=ok");

      // ── Connection attempt guard ──────────────────────────────────────
      namespace conn_attempt = ecnuvpn::connection_attempt;
      conn_attempt::AcquireOptions attempt_opts;
      attempt_opts.config_dir = utils::get_config_dir();
      attempt_opts.mode = "native";
      attempt_opts.owner_pid = conn_attempt::current_process_id();
      conn_attempt::AcquireResult attempt_result = conn_attempt::try_acquire(attempt_opts);
      timing.mark("connection_attempt",
                  attempt_result.acquired ? "acquired=true" : ("acquired=false code=" + attempt_result.code));

      if (!attempt_result.acquired) {
        nlohmann::json details;
        details["lock_path"] = utils::get_config_dir() + "/connect-attempt.lock";
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
          conn_attempt::mark_terminal(utils::get_config_dir(), "stale_auto_cleanup");
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
          utils::get_config_dir(), attempt_result.record.attempt_id, "scope_exit");

      // ── TunnelController connect ──────────────────────────────────────
      auto controller = ensure_tunnel_controller();
      timing.mark("tunnel_controller",
                  controller ? "initialized=true" : "initialized=false");

      if (!controller) {
        nlohmann::json fallback =
            platform::try_connect_direct_fallback(cfg, password);
        if (allow_direct_fallback && fallback.is_object() &&
            !fallback.empty()) {
          if (fallback.value("ok", false)) {
            timing.finish(true, "stage=direct_fallback");
            return frontend_status_from_snapshot_json(
                fallback.value("_snapshot_data", nlohmann::json::object()), cfg);
          }
          timing.finish(false, "stage=direct_fallback error=" +
                                   json_string(fallback, "error"));
          return fallback;
        }
        timing.finish(false, "stage=tunnel_controller_init");
        return error("Failed to initialize VPN controller: " +
                         tunnel_holder().init_error,
                     platform::kHelperUnavailableCode);
      }

      controller->set_vpn_config(cfg, password);
      cleanup_legacy_supervisor_state_files();
      timing.mark("cleanup_legacy_state");

      exv::core::UserIntent intent;
      intent.desired_connected = true;
      intent.auto_reconnect = cfg.auto_reconnect;
      intent.profile_id.value = cfg.server;
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
      } else {
        nlohmann::json fallback = platform::try_disconnect_direct_fallback(
            payload.value("allow_direct_fallback", false));
        if (fallback.is_object() && !fallback.empty()) {
          if (!fallback.value("ok", false))
            return fallback;
        }
      }
      cleanup_legacy_supervisor_state_files();
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
        if (!utils::trim(value).empty()) {
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
      if (payload.contains("webui_port") && payload["webui_port"].is_number_integer()) {
        Config updated = mgr.load();
        updated.webui_port = payload["webui_port"].get<int>();
        mgr.save(updated);
      }
      if (payload.contains("webui_host") && payload["webui_host"].is_string()) {
        Config updated = mgr.load();
        updated.webui_bind = payload["webui_host"].get<std::string>();
        mgr.save(updated);
      }
      if (payload.contains("webui_enabled") && payload["webui_enabled"].is_boolean()) {
        std::string err = config_api::config_set(mgr, "webui_enabled",
                               payload["webui_enabled"].get<bool>() ? "true" : "false");
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

nlohmann::json handle_action(const std::string &action,
                             const nlohmann::json &payload) {
  try {
    return desktop_adapter().dispatch(action, payload);
  } catch (const std::exception &ex) {
    return error(ex.what());
  } catch (...) {
    return error("Unknown desktop API error");
  }
}

bool is_tunnel_controller_active() {
  return exv::core::is_tunnel_controller_active();
}

} // namespace app_api
} // namespace ecnuvpn
