#include "app_api.hpp"

#include "config.hpp"
#include "config_api.hpp"
#include "config_manager.hpp"
#include "crypto.hpp"
#include "helper.hpp"
#include "logger.hpp"
#include "platform/common/app_api_runtime_policy.hpp"
#include "platform/common/backend_resolver.hpp"
#include "platform/common/helper_client.hpp"
#include "platform/common/driver_status.hpp"
#include "platform/common/oneshot_bootstrap.hpp"
#include "platform/common/runtime_status.hpp"
#include "platform/common/service_status.hpp"
#include "utils.hpp"
#include "virtual_network.hpp"

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
  nlohmann::json result{{"ok", false}, {"error", message}};
  if (!code.empty())
    result["code"] = code;
  return result;
}

std::string json_string(const nlohmann::json &object, const char *key,
                        const std::string &fallback = std::string());

bool helper_unavailable(const nlohmann::json &response) {
  return json_string(response, "code") == platform::kHelperUnavailableCode ||
         json_string(response, "message") == "Helper daemon not available";
}

class StageTimer {
public:
  explicit StageTimer(std::string scope)
      : scope_(std::move(scope)), started_(Clock::now()), last_(started_) {
    logger::info("[connect-timing] scope=" + scope_ +
                 " stage=begin delta_ms=0 total_ms=0");
  }

  void mark(const std::string &stage, const std::string &detail = "") {
    auto now = Clock::now();
    long long delta_ms = elapsed_ms(last_, now);
    long long total_ms = elapsed_ms(started_, now);
    last_ = now;

    std::string message = "[connect-timing] scope=" + scope_ +
                          " stage=" + stage +
                          " delta_ms=" + std::to_string(delta_ms) +
                          " total_ms=" + std::to_string(total_ms);
    if (!detail.empty())
      message += " " + detail;
    logger::info(message);
  }

  void finish(bool ok, const std::string &detail = "") {
    if (finished_)
      return;
    finished_ = true;
    mark(ok ? "finish.ok" : "finish.error", detail);
  }

private:
  using Clock = std::chrono::steady_clock;

  static long long elapsed_ms(const Clock::time_point &from,
                              const Clock::time_point &to) {
    return std::chrono::duration_cast<std::chrono::milliseconds>(to - from)
        .count();
  }

  std::string scope_;
  Clock::time_point started_;
  Clock::time_point last_;
  bool finished_ = false;
};

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
  // The desktop / RPC entrypoint does not go through the CLI wizard, so we
  // have to make sure the encryption key file exists before any password
  // operation. Without this, config_set_password would always fail with
  // "Encryption key is missing" on a fresh install.
  crypto::init_key_if_needed();
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
                        {"openconnect_runtime", cfg.openconnect_runtime},
                        {"windows_tunnel_driver", cfg.windows_tunnel_driver},
                        {"windows_tap_interface", cfg.windows_tap_interface}};
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

  if (!allow_direct_fallback) {
    platform::BackendResolveOptions options;
    options.preferred_mode = "service";
    options.allow_oneshot = false;
    options.allow_service_start = false;
    nlohmann::json backend = platform::resolve_backend(options);
    if (!backend.value("ok", false)) {
      return platform::backend_unavailable_error(
          backend, platform::helper_unavailable_connect_message());
    }
  }

  nlohmann::json runtime = runtime_status_json(cfg);
  if (!runtime.value("available", false)) {
    return error("OpenConnect runtime is not available. The desktop bundle is missing openconnect and its native dependencies.");
  }

  nlohmann::json platform_err = platform::preflight_connect_platform_checks(cfg);
  if (platform_err.is_object() && platform_err.value("ok", true) == false)
    return platform_err;

  return nlohmann::json{{"ok", true}};
}

nlohmann::json logs_json(const nlohmann::json &payload) {
  config::ConfigManager mgr = make_config_manager();
  Config cfg = mgr.load();
  std::string log_path = utils::expand_home(cfg.log_file);
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

nlohmann::json handle_action(const std::string &action,
                             const nlohmann::json &payload) {
  try {
    apply_desktop_runtime_context(payload);

    config::ConfigManager mgr = make_config_manager();
    Config cfg = mgr.load();

    if (action == "status.get") {
      auto helper_resp = platform::send_helper_request({{"action", "status"}});
      if (!json_bool(helper_resp, "ok", false) && helper_unavailable(helper_resp)) {
        nlohmann::json fallback = platform::status_fallback_without_helper(cfg);
        if (json_bool(fallback, "_snapshot", false)) {
          if (json_bool(fallback, "_running", false)) {
            return frontend_status_from_snapshot_json(
                fallback.contains("_snapshot_data")
                    ? fallback["_snapshot_data"]
                    : nlohmann::json::object(),
                cfg);
          }
          return disconnected_status(cfg);
        }
        return disconnected_status(cfg);
      }
      return frontend_status_from_helper(helper_resp, cfg);
    }

    if (action == "vpn.connect") {
      StageTimer timing("desktop.connect");
      std::string password = payload.value("password", std::string());
      bool allow_direct_fallback =
          payload.value("allow_direct_fallback", false);
      if (password.empty() && !cfg.password.empty()) {
        std::string key = crypto::load_key();
        if (!key.empty())
          password = crypto::decrypt(cfg.password, key);
      }
      timing.mark("password_resolved",
                  password.empty() ? "source=missing" : "source=available");
      nlohmann::json preflight =
          preflight_connect(cfg, password, allow_direct_fallback);
      if (preflight.is_object() && preflight.value("ok", true) == false) {
        timing.finish(false, "stage=preflight error=" +
                                 json_string(preflight, "error"));
        return preflight;
      }
      timing.mark("preflight", "result=ok allow_direct_fallback=" +
                                   std::string(allow_direct_fallback ? "true"
                                                                     : "false"));

      bool helper_available = helper::is_available();
      timing.mark("helper_availability",
                  helper_available ? "available=true" : "available=false");

      if (allow_direct_fallback && !helper_available) {
        nlohmann::json fallback = platform::status_fallback_without_helper(cfg);
        timing.mark("direct_fallback_status",
                    json_bool(fallback, "_running", false) ? "running=true"
                                                           : "running=false");
        if (json_bool(fallback, "_snapshot", false) &&
            json_bool(fallback, "_running", false)) {
          nlohmann::json status = frontend_status_from_snapshot_json(
              fallback.contains("_snapshot_data")
                  ? fallback["_snapshot_data"]
                  : nlohmann::json::object(),
              cfg);
          status["mode"] = "elevated";
          timing.finish(true, "mode=elevated existing=true");
          return status;
        }
      }

      if (!helper_available && allow_direct_fallback) {
        platform::BackendResolveOptions options;
        options.preferred_mode = "oneshot";
        options.helper_path = helper_binary_next_to_exv();
        options.allow_oneshot = true;
        options.allow_service_start = false;
        options.start_oneshot = true;
        nlohmann::json backend = platform::resolve_backend(options);
        timing.mark("oneshot_backend",
                    backend.value("ok", false) ? "result=ok" : "result=failed");
        if (backend.value("ok", false)) {
          platform::HelperEndpoint endpoint{
              backend.value("endpoint", std::string()),
              backend.value("auth_token", std::string())};
          nlohmann::json helper_resp = platform::send_helper_request(
              endpoint,
              [&] {
                nlohmann::json request{{"action", "start"},
                                       {"config", cfg},
                                       {"password", password},
                                       {"retry_limit", 0},
                                       {"home", utils::get_effective_home()},
                                       {"config_dir", utils::get_config_dir()}};
                 add_desktop_owner_context(request);
                 return request;
               }());
          timing.mark("oneshot_helper_request",
                      helper_resp.value("ok", false) ? "result=ok"
                                                     : "result=failed");
          if (!helper_resp.value("ok", false)) {
            timing.finish(false, "stage=oneshot_helper_request");
            return helper_error(helper_resp, "Failed to start VPN");
          }
          nlohmann::json status = frontend_status_from_helper(helper_resp, cfg);
          status["mode"] = "elevated";
          status["backend"] = backend;
          timing.finish(true,
                        "mode=elevated network_ready=" +
                            std::string(status.value("network_ready", false)
                                            ? "true"
                                            : "false"));
          return status;
        }
        timing.finish(false, "stage=oneshot_backend");
        return platform::backend_unavailable_error(
            backend, "Failed to start one-shot helper.");
      }

      nlohmann::json start_request{{"action", "start"},
                                   {"config", cfg},
                                   {"password", password},
                                   {"retry_limit", 0},
                                   {"home", utils::get_effective_home()},
                                   {"config_dir", utils::get_config_dir()}};
      add_desktop_owner_context(start_request);
      auto helper_resp = platform::send_helper_request(start_request);
      timing.mark("service_helper_request",
                  helper_resp.value("ok", false) ? "result=ok"
                                                 : "result=failed");
      if (!helper_resp.value("ok", false)) {
        timing.finish(false, "stage=service_helper_request");
        return helper_error(helper_resp, "Failed to start VPN");
      }
      nlohmann::json status = frontend_status_from_helper(helper_resp, cfg);
      timing.finish(true,
                    "mode=helper network_ready=" +
                        std::string(status.value("network_ready", false)
                                        ? "true"
                                        : "false"));
      return status;
    }

    if (action == "vpn.disconnect") {
      bool allow_direct_fallback =
          payload.value("allow_direct_fallback", false);
      nlohmann::json backend =
          payload.value("backend", nlohmann::json::object());
      if (backend.is_object() &&
          backend.value("backend", std::string()) == "oneshot") {
        platform::HelperEndpoint endpoint{
            backend.value("endpoint", std::string()),
            backend.value("auth_token", std::string())};
        auto helper_resp =
            platform::send_helper_request(endpoint, {{"action", "stop"}});
        if (!helper_resp.value("ok", false)) {
          return helper_error(helper_resp, "Failed to stop VPN");
        }
        return disconnected_status(cfg);
      }

      auto helper_resp = platform::send_helper_request({{"action", "stop"}});
      if (!helper_resp.value("ok", false) && helper_unavailable(helper_resp)) {
        if (!allow_direct_fallback) {
          return error(platform::helper_unavailable_disconnect_message(),
                       platform::kHelperUnavailableCode);
        }
        nlohmann::json fallback =
            platform::try_disconnect_direct_fallback(allow_direct_fallback);
        if (fallback.is_object() && !fallback.empty()) {
          if (!fallback.value("ok", false)) {
            return error(fallback.value("error", "Failed to stop VPN"));
          }
          return disconnected_status(cfg);
        }
        return error("One-shot helper is no longer running.",
                     platform::kHelperUnavailableCode);
      }
      if (!helper_resp.value("ok", false)) {
        return helper_error(helper_resp, "Failed to stop VPN");
      }
      return disconnected_status(cfg);
    }

    if (action == "config.getAuth")
      return auth_config(cfg);

    if (action == "config.saveAuth") {
      if (payload.contains("server") && payload["server"].is_string()) {
        std::string err = config_api::config_set(mgr, "server", payload["server"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      if (payload.contains("username") && payload["username"].is_string()) {
        std::string err = config_api::config_set(mgr, "username", payload["username"].get<std::string>());
        if (!err.empty()) return error(err);
      }
      // Update remember_password BEFORE password so that the password setter
      // sees the correct toggle state. The UI treats an empty password field
      // as "keep the existing password" while a non-empty value means update.
      if (payload.contains("remember_password") &&
          payload["remember_password"].is_boolean()) {
        std::string err = config_api::config_set(mgr, "remember_password",
                               payload["remember_password"].get<bool>() ? "true"
                                                                         : "false");
        if (!err.empty()) return error(err);
      }
      if (payload.contains("password") && payload["password"].is_string()) {
        std::string password = payload["password"].get<std::string>();
        if (!password.empty()) {
          std::string err = config_api::config_set_password(mgr, password);
          if (!err.empty())
            return error(err);
        }
      }
      if (payload.contains("user_agent") && payload["user_agent"].is_string()) {
        std::string value = payload["user_agent"].get<std::string>();
        // Treat empty / whitespace user_agent as "no change" so the UI cannot
        // accidentally overwrite the platform default.
        if (!utils::trim(value).empty()) {
          std::string err = config_api::config_set(mgr, "useragent", value);
          if (!err.empty()) return error(err);
        }
      }
      return auth_config(mgr.load());
    }

    if (action == "config.getSettings")
      return settings_config(cfg);

    if (action == "config.saveSettings") {
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
    }

    if (action == "config.getKey")
      return key_status_json();

    if (action == "routes.list")
      return routes_json(cfg);

    if (action == "routes.add") {
      std::string err = config_api::route_add(mgr, payload.value("cidr", ""));
      if (!err.empty())
        return error(err);
      return routes_json(mgr.load());
    }

    if (action == "routes.remove") {
      std::string err = config_api::route_remove(mgr, payload.value("cidr", ""));
      if (!err.empty())
        return error(err);
      return routes_json(mgr.load());
    }

    if (action == "routes.reset") {
      config_api::route_reset_defaults(mgr);
      return routes_json(mgr.load());
    }

    if (action == "service.status")
      return service_status_json();

    if (action == "helper.status")
    {
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
    }

    if (action == "runtime.status")
      return runtime_status_json(cfg);

    if (action == "drivers.status")
      return driver_status_json(cfg);

    if (action == "drivers.install")
      return install_driver(cfg, payload);

    if (action == "logs.list")
      return logs_json(payload);

    return error("Unknown desktop action: " + action);
  } catch (const std::exception &ex) {
    return error(ex.what());
  } catch (...) {
    return error("Unknown desktop API error");
  }
}

} // namespace app_api
} // namespace ecnuvpn
