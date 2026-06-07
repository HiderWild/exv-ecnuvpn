#include "helper.hpp"
#include "helper_ipc.hpp"

#include "core/timing.hpp"
#include "helper_common/helper_messages.hpp"
#include "helper_v2_handler.hpp"
#include "logger.hpp"
#include "runtime/runtime_context.hpp"
#include "feedback/feedback.hpp"
#include "platform/common/helper_client.hpp"
#include "platform/common/helper_lifecycle.hpp"
#include "platform/common/helper_platform.hpp"
#include "platform/common/helper_service_manager.hpp"
#include "utils.hpp"
#include "vpn.hpp"
#include "virtual_network.hpp"
#include "vpn_engine/native_error_contract.hpp"
#include "vpn_engine/native_session_store.hpp"

#include <cerrno>
#include <cctype>
#include <csignal>
#include <cstring>
#include <cstdio>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <utility>
#include <vector>


namespace ecnuvpn {
namespace helper {

namespace {

volatile sig_atomic_t daemon_stop_requested = 0;
DaemonOptions active_daemon_options;

using StageTimer = exv::core::ConnectStageTimer;

struct SessionState {
  uid_t uid = static_cast<uid_t>(-1);
  gid_t gid = static_cast<gid_t>(-1);
  std::string engine = "legacy_openconnect";
  std::string username;
  std::string home;
  std::string config_dir;
  std::string server;
  int route_count = 0;
  int retry_limit = 0; // DEPRECATED: V1 legacy. ReconnectPolicy in Core owns this.
};

struct RuntimeSnapshot {
  bool running = false;
  pid_t pid = -1;
  pid_t supervisor_pid = -1;
  bool network_ready = false;
  std::string interface_name;
  uint64_t rx_bytes = 0;
  uint64_t tx_bytes = 0;
  std::chrono::steady_clock::time_point last_traffic_update{};
  std::string internal_ip;
  std::string interfaces_output;
};

std::string pid_path_for(const SessionState &state) {
  return state.config_dir + "/ecnuvpn.pid";
}

std::string supervisor_pid_path_for(const SessionState &state) {
  return state.config_dir + "/ecnuvpn-supervisor.pid";
}

std::string route_ready_path_for(const SessionState &state) {
  return state.config_dir + "/route-ready";
}

bool is_native_session(const SessionState &state) {
  return state.engine == "native";
}

void remove_file_if_exists(const std::string &path) {
  if (utils::file_exists(path)) {
    std::remove(path.c_str());
  }
}

void clear_runtime_state(const SessionState &state) {
  if (state.config_dir.empty())
    return;
  remove_file_if_exists(pid_path_for(state));
  remove_file_if_exists(supervisor_pid_path_for(state));
  if (is_native_session(state)) {
    vpn_engine::clear_native_session_state(state.config_dir);
  } else {
    remove_file_if_exists(route_ready_path_for(state));
  }
}

pid_t read_pid_file(const std::string &path) {
  if (!utils::file_exists(path))
    return -1;
  std::string content = utils::trim(utils::read_file(path));
  if (content.empty())
    return -1;
  try {
    return static_cast<pid_t>(std::stoi(content));
  } catch (...) {
    return -1;
  }
}

bool is_process_alive(int pid) { return platform::is_process_alive(pid); }

int find_openconnect_pid() { return platform::find_openconnect_pid(); }

bool read_route_ready(const SessionState &state, std::string *interface_name,
                      std::string *internal_ip) {
  std::string path = route_ready_path_for(state);
  if (!utils::file_exists(path))
    return false;

  std::istringstream iss(utils::read_file(path));
  std::string tun;
  std::string ip;
  if (!std::getline(iss, tun) || !std::getline(iss, ip))
    return false;

  tun = utils::trim(tun);
  ip = utils::trim(ip);
  if (tun.empty() || ip.empty())
    return false;

  if (interface_name)
    *interface_name = tun;
  if (internal_ip)
    *internal_ip = ip;
  return true;
}

nlohmann::json to_json(const SessionState &state) {
  return nlohmann::json{{"uid", static_cast<unsigned int>(state.uid)},
                        {"gid", static_cast<unsigned int>(state.gid)},
                        {"engine", state.engine},
                        {"username", state.username},
                        {"home", state.home},
                        {"config_dir", state.config_dir},
                        {"server", state.server},
                        {"route_count", state.route_count},
                        {"retry_limit", state.retry_limit}};
}

bool from_json(const nlohmann::json &j, SessionState *state) {
  if (!state)
    return false;
  try {
    state->uid = static_cast<uid_t>(j.at("uid").get<unsigned int>());
    state->gid = static_cast<gid_t>(j.at("gid").get<unsigned int>());
    state->engine = j.value("engine", std::string("legacy_openconnect"));
    state->username = j.at("username").get<std::string>();
    state->home = j.at("home").get<std::string>();
    state->config_dir = j.at("config_dir").get<std::string>();
    state->server = j.value("server", std::string());
    state->route_count = j.value("route_count", 0);
    state->retry_limit = j.value("retry_limit", 0);
    return true;
  } catch (...) {
    return false;
  }
}

bool save_session_state(const SessionState &state) {
  const auto &platform_config = platform::helper_platform_config();
  std::ofstream ofs(platform_config.session_state_path);
  if (!ofs.is_open())
    return false;
  ofs << to_json(state).dump(2);
  ofs.close();
  platform::set_session_state_permissions(platform_config.session_state_path);
  return ofs.good();
}

bool load_session_state(SessionState *state) {
  const auto &platform_config = platform::helper_platform_config();
  if (!state || !utils::file_exists(platform_config.session_state_path))
    return false;
  try {
    nlohmann::json j =
        nlohmann::json::parse(utils::read_file(platform_config.session_state_path));
    return from_json(j, state);
  } catch (...) {
    return false;
  }
}

void clear_session_state() {
  remove_file_if_exists(platform::helper_platform_config().session_state_path);
}

void clear_native_session_state_for_known_config_dirs(uid_t peer_uid) {
  std::vector<std::string> config_dirs;
  const std::string peer_config_dir = utils::get_config_dir_for_uid(peer_uid);
  if (!peer_config_dir.empty())
    config_dirs.push_back(peer_config_dir);

  const std::string helper_config_dir = utils::get_config_dir();
  if (!helper_config_dir.empty() && helper_config_dir != peer_config_dir)
    config_dirs.push_back(helper_config_dir);

  vpn_engine::clear_native_session_states(config_dirs);
}

RuntimeSnapshot inspect_runtime(const SessionState &state) {
  RuntimeSnapshot snapshot;
  if (state.config_dir.empty())
    return snapshot;

  if (is_native_session(state)) {
    vpn_engine::NativeSessionProbe probe;
    probe.is_process_alive = [](int pid) { return is_process_alive(pid); };
    vpn_engine::NativeSessionSnapshot native =
        vpn_engine::read_native_session_snapshot(state.config_dir, probe);

    snapshot.running = native.running;
    snapshot.pid = static_cast<pid_t>(native.pid);
    snapshot.supervisor_pid = static_cast<pid_t>(native.supervisor_pid);
    snapshot.network_ready = native.network_ready;
    snapshot.interface_name = native.interface_name;
    snapshot.internal_ip = native.internal_ip;
    if (snapshot.running)
      snapshot.interfaces_output = platform::get_interfaces_output();
    return snapshot;
  }

  snapshot.supervisor_pid = read_pid_file(supervisor_pid_path_for(state));
  if (!is_process_alive(snapshot.supervisor_pid))
    snapshot.supervisor_pid = -1;

  snapshot.pid = read_pid_file(pid_path_for(state));
  if (!is_process_alive(snapshot.pid))
    snapshot.pid = -1;

  if (snapshot.pid <= 0)
    snapshot.pid = find_openconnect_pid();

  snapshot.network_ready =
      read_route_ready(state, &snapshot.interface_name, &snapshot.internal_ip);
  snapshot.running = snapshot.pid > 0 || snapshot.supervisor_pid > 0;

  if (snapshot.running) {
    snapshot.interfaces_output = platform::get_interfaces_output();
  }

  return snapshot;
}

bool create_request_file(const nlohmann::json &request, std::string *out_path) {
  if (!out_path)
    return false;

  std::string payload = request.dump();
  std::string path = platform::create_temp_request_file(payload);
  if (path.empty())
    return false;

  *out_path = path;
  return true;
}

void daemon_signal_handler(int) {
  daemon_stop_requested = 1;
}

bool send_request(const nlohmann::json &request, nlohmann::json *response,
                  std::string *error_message = nullptr,
                  int /*timeout_seconds*/ = 15) {
  nlohmann::json result = platform::send_helper_request(request);

  if (!result.is_object()) {
    if (error_message)
      *error_message = "Failed to parse EXV helper response.";
    return false;
  }

  if (result.contains("ok") && result["ok"].is_boolean() && !result["ok"].get<bool>()) {
    if (result.contains("code") && result["code"].is_string() &&
        result["code"].get<std::string>() == std::string(platform::kHelperUnavailableCode)) {
      if (error_message)
        *error_message = "EXV helper is not available.";
    } else {
      if (error_message)
        *error_message = result.value("message", std::string("EXV helper request failed."));
    }
    return false;
  }

  if (response)
    *response = result;
  return true;
}

bool wait_until_available(int attempts = 1, useconds_t delay_us = 0) {
  for (int i = 0; i < attempts; ++i) {
    nlohmann::json response;
    std::string error_message;
    if (send_request(nlohmann::json{{"action", "status"}}, &response,
                     &error_message)) {
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

nlohmann::json make_error(const std::string &message,
                          const std::string &code = std::string()) {
  // Route every helper error through the unified feedback module so it always
  // carries a canonical, non-empty code plus recoverable/recommended_action.
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
#ifdef _WIN32
                        {"platform", "windows"},
                        {"transport", "named-pipe"},
#elif defined(__APPLE__)
                        {"platform", "darwin"},
                        {"transport", "unix-socket"},
#elif defined(__linux__)
                        {"platform", "linux"},
                        {"transport", "unix-socket"},
#else
                        {"platform", "unknown"},
                        {"transport", "unknown"},
#endif
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

nlohmann::json make_status_response(const SessionState &state,
                                    const RuntimeSnapshot &snapshot,
                                    bool ok = true,
                                    const std::string &message = "") {
  nlohmann::json response{{"ok", ok},
                          {"message", message},
                          {"running", snapshot.running},
                          {"pid", snapshot.pid},
                          {"supervisor_pid", snapshot.supervisor_pid},
                          {"network_ready", snapshot.network_ready},
                          {"interface", snapshot.interface_name},
                          {"internal_ip", snapshot.internal_ip},
                          {"rx_bytes", snapshot.rx_bytes},
                          {"tx_bytes", snapshot.tx_bytes},
                          {"engine", state.engine},
                          {"server", state.server},
                          {"route_count", state.route_count},
                          {"retry_limit", state.retry_limit},
                          {"owner_username", state.username},
                          {"interfaces_output", snapshot.interfaces_output}};
  virtual_network::add_status_fields(response, snapshot.interface_name);
  add_helper_descriptor_fields(response);
  return response;
}

bool ensure_same_owner(const SessionState &state, uid_t peer_uid) {
  // Any local user who can reach the helper socket may manage the VPN session.
  // Any local user who can reach the helper socket may manage the VPN session.
  // Socket is mode 0660, group staff (gid 20) â€?access controlled at OS level.
  (void)state;
  (void)peer_uid;
  return true;
}

nlohmann::json handle_status(uid_t peer_uid) {
  SessionState state;
  if (!load_session_state(&state)) {
    nlohmann::json response{{"ok", true}, {"running", false}};
    add_helper_descriptor_fields(response);
    return response;
  }

  if (!ensure_same_owner(state, peer_uid)) {
    return make_error("VPN session belongs to another local user.");
  }

  RuntimeSnapshot snapshot = inspect_runtime(state);
  if (!snapshot.running) {
    platform::cleanup_routes();
    clear_runtime_state(state);
    clear_session_state();
    nlohmann::json response{{"ok", true}, {"running", false}};
    add_helper_descriptor_fields(response);
    return response;
  }

  return make_status_response(state, snapshot);
}

bool stop_managed_session(const SessionState &state, std::string *message) {
  if (is_native_session(state)) {
    RuntimeSnapshot snapshot = inspect_runtime(state);
    if (!snapshot.running) {
      platform::cleanup_routes();
      platform::kill_all_supervisors();
      clear_runtime_state(state);
      if (message)
        *message = "No native VPN session found. VPN is not running.";
      return false;
    }

    platform::cleanup_routes();

    if (snapshot.supervisor_pid > 0)
      platform::terminate_process(static_cast<int>(snapshot.supervisor_pid));
    if (snapshot.pid > 0)
      platform::terminate_process(static_cast<int>(snapshot.pid));

    for (int i = 0; i < 10; ++i) {
      platform::sleep_ms(300);
      if ((snapshot.pid <= 0 || !is_process_alive(snapshot.pid)) &&
          (snapshot.supervisor_pid <= 0 ||
           !is_process_alive(snapshot.supervisor_pid))) {
        break;
      }
    }

    if (snapshot.pid > 0 && is_process_alive(snapshot.pid))
      platform::force_terminate_process(static_cast<int>(snapshot.pid));
    if (snapshot.supervisor_pid > 0 &&
        is_process_alive(snapshot.supervisor_pid))
      platform::force_terminate_process(
          static_cast<int>(snapshot.supervisor_pid));

    platform::sleep_ms(500);

    if ((snapshot.pid > 0 && is_process_alive(snapshot.pid)) ||
        (snapshot.supervisor_pid > 0 &&
         is_process_alive(snapshot.supervisor_pid))) {
      if (message)
        *message = "Failed to stop native VPN session!";
      return false;
    }

    platform::kill_all_supervisors();
    clear_runtime_state(state);
    if (message)
      *message = "VPN connection stopped successfully! ðŸŽ‰";
    return true;
  }

  pid_t supervisor_pid = read_pid_file(supervisor_pid_path_for(state));
  if (!is_process_alive(supervisor_pid))
    supervisor_pid = -1;

  pid_t pid = read_pid_file(pid_path_for(state));
  if (!is_process_alive(pid))
    pid = -1;

  if (pid <= 0)
    pid = find_openconnect_pid();

  if (pid <= 0 && supervisor_pid <= 0) {
    platform::cleanup_routes();
    platform::kill_all_supervisors();
    clear_runtime_state(state);
    if (message)
      *message = "No openconnect process found. VPN is not running.";
    return false;
  }

  // Clean up routes before killing openconnect â€?while the tunnel
  // interface is still valid, route deletion is more reliable.
  platform::cleanup_routes();

  if (supervisor_pid > 0)
    platform::terminate_process(supervisor_pid);
  if (pid > 0)
    platform::terminate_process(pid);

  for (int i = 0; i < 10; ++i) {
    platform::sleep_ms(300);
    if ((pid <= 0 || !is_process_alive(pid)) &&
        (supervisor_pid <= 0 || !is_process_alive(supervisor_pid))) {
      break;
    }
  }

  if (pid > 0 && is_process_alive(pid))
    platform::force_terminate_process(pid);
  if (supervisor_pid > 0 && is_process_alive(supervisor_pid))
    platform::force_terminate_process(supervisor_pid);

  platform::sleep_ms(500);

  pid_t remaining_pid = find_openconnect_pid();
  if (remaining_pid > 0 && remaining_pid != pid) {
    platform::terminate_process(remaining_pid);
    platform::sleep_ms(500);
    if (is_process_alive(remaining_pid))
      platform::force_terminate_process(remaining_pid);
  }

  if ((pid > 0 && is_process_alive(pid)) ||
      (remaining_pid > 0 && is_process_alive(remaining_pid)) ||
      (supervisor_pid > 0 && is_process_alive(supervisor_pid))) {
    if (message)
      *message = "Failed to stop VPN connection!";
    return false;
  }

  platform::kill_all_supervisors();

  clear_runtime_state(state);
  if (message)
    *message = "VPN connection stopped successfully! ðŸŽ‰";
  return true;
}

nlohmann::json handle_stop(uid_t peer_uid) {
  SessionState state;
  if (!load_session_state(&state)) {
    // No session state: still clean up orphaned native/legacy runtime state.
    platform::cleanup_routes();
    platform::kill_all_supervisors();
    clear_native_session_state_for_known_config_dirs(peer_uid);
    return make_error("No openconnect process found. VPN is not running.");
  }

  if (!ensure_same_owner(state, peer_uid)) {
    return make_error("VPN session belongs to another local user.");
  }

  std::string message;
  bool ok = stop_managed_session(state, &message);
  if (ok) {
    clear_session_state();
    return nlohmann::json{{"ok", true}, {"message", message}};
  }

  // stop_managed_session failed â€?belt-and-suspenders cleanup
  platform::cleanup_routes();
  platform::kill_all_supervisors();
  RuntimeSnapshot snapshot = inspect_runtime(state);
  if (!snapshot.running)
    clear_session_state();
  return make_error(message);
}

nlohmann::json handle_start(uid_t peer_uid, gid_t peer_gid,
                            const nlohmann::json &request) {
  StageTimer timing("helper.start");
  SessionState existing;
  if (load_session_state(&existing)) {
    RuntimeSnapshot current = inspect_runtime(existing);
    timing.mark("existing_session_checked",
                current.running ? "running=true" : "running=false");
    if (current.running) {
      if (!ensure_same_owner(existing, peer_uid)) {
        timing.finish(false, "reason=owned_by_another_user");
        return make_error("VPN session belongs to another local user.");
      }
      timing.finish(false, "reason=already_running");
      return nlohmann::json{{"ok", false},
                            {"message", "VPN is already running."},
                            {"running", true},
                            {"pid", current.pid},
                            {"supervisor_pid", current.supervisor_pid}};
    }
    clear_runtime_state(existing);
    clear_session_state();
  } else {
    timing.mark("existing_session_checked", "running=false");
  }

  Config cfg;
  std::string plaintext_password;
  int retry_limit = 0;
  std::string requested_home;
  std::string requested_config_dir;
  try {
    cfg = request.at("config").get<Config>();
    plaintext_password = request.at("password").get<std::string>();
    retry_limit = request.value("retry_limit", 0);
    requested_home = request.value("home", std::string());
    requested_config_dir = request.value("config_dir", std::string());
  } catch (...) {
    timing.finish(false, "reason=invalid_payload");
    return make_error("Invalid start request payload.");
  }

  // Native engine must use TunnelController + HelperClient V2 session-based
  // API, not the legacy V1 start path that leaks plaintext password.
  if (cfg.vpn_engine == "native") {
    timing.finish(false, "reason=native_engine_rejected");
    return nlohmann::json{{"ok", false},
                          {"code", "native_engine_not_supported"},
                          {"message",
                           "Native engine connections must use the V2 helper "
                           "session API (TunnelController). The legacy V1 "
                           "\"start\" action does not accept native engine "
                           "requests."}};
  }

  timing.mark("request_parsed",
              "routes=" + std::to_string(cfg.routes.size()) +
                  " retry_limit=" + std::to_string(retry_limit));

  SessionState state;
  state.uid = static_cast<uid_t>(request.value(
      "owner_uid", static_cast<unsigned int>(peer_uid)));
  state.gid = static_cast<gid_t>(request.value(
      "owner_gid", static_cast<unsigned int>(peer_gid)));
  state.username = utils::get_username_for_uid(state.uid);
  state.home = requested_home.empty() ? utils::get_home_for_uid(peer_uid)
                                    : requested_home;
  state.config_dir =
      requested_config_dir.empty() ? utils::get_config_dir_for_uid(peer_uid)
                                   : requested_config_dir;
  state.engine = cfg.vpn_engine.empty() ? std::string("legacy_openconnect")
                                        : cfg.vpn_engine;
  state.server = cfg.server;
  state.route_count = static_cast<int>(cfg.routes.size());
  state.retry_limit = retry_limit;
  timing.mark("session_prepared",
              "server=" + state.server +
                  " routes=" + std::to_string(state.route_count));

  nlohmann::json worker_request{{"uid", static_cast<unsigned int>(state.uid)},
                                {"gid", static_cast<unsigned int>(state.gid)},
                                {"home", state.home},
                                {"config_dir", state.config_dir},
                                {"retry_limit", retry_limit},
                                {"password", plaintext_password},
                                {"config", cfg}};

  std::string request_path;
  if (!create_request_file(worker_request, &request_path)) {
    timing.finish(false, "reason=request_file_failed");
    return make_error("Failed to prepare helper request file.");
  }
  timing.mark("worker_request_file_created");

  std::string executable_path = utils::get_executable_path();
  int status = platform::spawn_worker_process(executable_path, request_path);
  timing.mark("worker_process_finished", "exit_status=" + std::to_string(status));
  if (status < 0) {
    remove_file_if_exists(request_path);
    timing.finish(false, "reason=worker_spawn_failed");
    return make_error("Failed to launch EXV helper worker.");
  }

  remove_file_if_exists(request_path);

  RuntimeSnapshot snapshot = inspect_runtime(state);
  timing.mark("runtime_inspected",
              "running=" + std::string(snapshot.running ? "true" : "false") +
                  " network_ready=" +
                  std::string(snapshot.network_ready ? "true" : "false") +
                  " pid=" + std::to_string(snapshot.pid));
  if (status == 0 && snapshot.running) {
    save_session_state(state);
    timing.mark("session_state_saved");
    if (snapshot.network_ready) {
      timing.finish(true, "network_ready=true");
      return make_status_response(state, snapshot, true,
                                  "VPN connected successfully!");
    }
    timing.finish(true, "network_ready=false");
    return make_status_response(
        state, snapshot, true,
        "VPN process started, but network routes are not ready yet.");
  }

  std::string native_failure_code;
  std::string native_failure_message;
  if (is_native_session(state)) {
    vpn_engine::NativeSessionProbe failure_probe;
    failure_probe.is_process_alive = [](int pid) {
      return is_process_alive(pid);
    };
    vpn_engine::NativeSessionSnapshot native_failure =
        vpn_engine::read_native_session_snapshot(state.config_dir,
                                                 failure_probe);
    if (!native_failure.failure_code.empty()) {
      native_failure_code = vpn_engine::map_native_error_to_contract_code(
          native_failure.failure_code, native_failure.failure_message);
      native_failure_message = native_failure.failure_message;
    }
    // Clear state now that we have captured the failure code. This was
    // previously cleared prematurely inside start_with_password (Bug 2).
    vpn_engine::clear_native_session_state(state.config_dir);
    remove_file_if_exists(pid_path_for(state));
    remove_file_if_exists(supervisor_pid_path_for(state));
  } else {
    clear_runtime_state(state);
  }
  clear_session_state();
  if (active_daemon_options.oneshot) {
    daemon_stop_requested = 1;
  }
  timing.finish(false, "reason=worker_failed");
  if (status == vpn::kVpnInitialConnectFailedExitCode) {
    logger::warn("Initial VPN connection failed before network was ready");
    return make_error("VPN authentication failed or the server rejected the connection.",
                      "auth_failed");
  }
  if (!native_failure_code.empty()) {
    logger::warn("Native VPN connection failed with code: " +
                 native_failure_code);
    return make_error(native_failure_message.empty()
                          ? "The native VPN engine failed to establish the connection."
                          : native_failure_message,
                      native_failure_code);
  }
  logger::event("ERROR", "helper", feedback::code::kConnectionFailed,
                "VPN connection failed without a specific engine failure code",
                {{"status", std::to_string(status)},
                 {"native", is_native_session(state) ? "true" : "false"}});
  return make_error("Failed to establish the VPN connection. Check logs with: exv logs");
}

nlohmann::json handle_request(uid_t peer_uid, gid_t peer_gid,
                              const nlohmann::json &request) {
  if (active_daemon_options.auth_required &&
      request.value("auth_token", std::string()) !=
          active_daemon_options.auth_token) {
    return nlohmann::json{{"ok", false},
                          {"code", "auth_failed"},
                          {"message", "Helper authentication failed."}};
  }

  std::string action = request.value("action", std::string());
  if (action == "hello")
    return make_hello_response();
  if (action == "start")
    return handle_start(peer_uid, peer_gid, request);
  if (action == "stop")
    return handle_stop(peer_uid);
  if (action == "status")
    return handle_status(peer_uid);
  return make_error("Unknown helper action.");
}

bool print_running_status(const nlohmann::json &response) {
  bool running = response.value("running", false);
  if (!running) {
    std::cout << utils::RED << utils::BOLD << "  â—?VPN is NOT RUNNING"
              << utils::RESET << std::endl;
    std::cout << std::endl;
    return true;
  }

  std::cout << utils::GREEN << utils::BOLD << "  â—?VPN is RUNNING"
            << utils::RESET << std::endl;
  std::cout << std::endl;

  int pid = response.value("pid", -1);
  int supervisor_pid = response.value("supervisor_pid", -1);
  if (pid > 0)
    std::cout << "  PID            : " << pid << std::endl;
  if (supervisor_pid > 0)
    std::cout << "  Supervisor PID : " << supervisor_pid << std::endl;
  std::cout << "  Network Ready  : "
            << (response.value("network_ready", false)
                    ? "yes"
                    : "no (waiting for tunnel script)")
            << std::endl;
  if (response.value("network_ready", false)) {
    std::cout << "  Interface      : "
              << response.value("interface", std::string()) << std::endl;
    std::cout << "  Internal IP    : "
              << response.value("internal_ip", std::string()) << std::endl;
  }
  if (response.value("upstream_virtual_detected", false)) {
    std::cout << "  Route Policy   : EXV campus routes first, upstream virtual adapter preserved"
              << std::endl;
    std::string message =
        response.value("upstream_virtual_message", std::string());
    if (!message.empty())
      std::cout << "  Notice         : " << message << std::endl;
  }

  std::string interfaces_output = response.value("interfaces_output", std::string());
  if (!interfaces_output.empty()) {
    std::cout << std::endl;
    std::cout << utils::DIM << "  Network Interfaces:" << utils::RESET
              << std::endl;
    std::istringstream iss(interfaces_output);
    std::string line;
    while (std::getline(iss, line)) {
      std::cout << "    " << line << std::endl;
    }
  }

  std::cout << std::endl;
  return true;
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

void clear_session_state_for_platform() { clear_session_state(); }

platform::HelperServiceManagerContext make_helper_service_manager_context() {
  return platform::HelperServiceManagerContext{
      wait_until_available_for_platform,
      send_request_for_platform,
      clear_session_state_for_platform,
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

bool start_via_helper(const Config &cfg, const std::string &plaintext_password,
                      int retry_limit) {
  StageTimer timing("helper.client.start");
  nlohmann::json response;
  std::string error_message;
  if (!send_request(nlohmann::json{{"action", "start"},
                                   {"config", cfg},
                                   {"password", plaintext_password},
                                   {"retry_limit", retry_limit},
                                    {"home", utils::get_effective_home()},
                                    {"config_dir", utils::get_config_dir()}},
                    &response, &error_message, 120)) {
    timing.finish(false, "stage=send_request");
    utils::print_error(error_message);
    return false;
  }
  timing.mark("send_request",
              response.value("ok", false) ? "result=ok" : "result=failed");

  if (!response.value("ok", false)) {
    timing.finish(false, "stage=helper_response");
    std::string message = response.value("message", std::string("Start failed."));
    if (message.find("already running") != std::string::npos) {
      utils::print_warning(message);
      int pid = response.value("pid", -1);
      int supervisor_pid = response.value("supervisor_pid", -1);
      if (pid > 0 || supervisor_pid > 0) {
        utils::print_info("Use 'exv stop' to stop the current connection first.");
      }
    } else {
      utils::print_error(message);
    }
    return false;
  }

  bool network_ready = response.value("network_ready", false);
  timing.finish(true,
                "network_ready=" +
                    std::string(network_ready ? "true" : "false"));
  std::cout << std::endl;
  if (network_ready) {
    utils::print_success("VPN connected successfully!");
  } else {
    utils::print_warning("VPN process started, but network routes are not ready yet.");
  }
  int pid = response.value("pid", -1);
  int supervisor_pid = response.value("supervisor_pid", -1);
  if (pid > 0)
    std::cout << utils::DIM << "  PID: " << pid << utils::RESET << std::endl;
  if (supervisor_pid > 0)
    std::cout << utils::DIM << "  Supervisor PID: " << supervisor_pid
              << utils::RESET << std::endl;
  if (network_ready) {
    std::cout << utils::DIM << "  Interface: "
              << response.value("interface", std::string()) << utils::RESET
              << std::endl;
    std::cout << utils::DIM << "  Internal IP: "
              << response.value("internal_ip", std::string()) << utils::RESET
              << std::endl;
  }
  std::cout << utils::DIM << "  Server: "
            << response.value("server", std::string()) << utils::RESET
            << std::endl;
  std::cout << utils::DIM << "  Routes: " << response.value("route_count", 0)
            << " configured" << utils::RESET << std::endl;
  if (response.value("upstream_virtual_detected", false)) {
    std::string message =
        response.value("upstream_virtual_message", std::string());
    if (!message.empty())
      utils::print_warning(message);
  }
  int helper_retry_limit = response.value("retry_limit", 0);
  if (helper_retry_limit != 0) {
    std::cout << utils::DIM << "  Auto-reconnect: "
              << (helper_retry_limit < 0
                      ? std::string("infinite")
                      : std::to_string(helper_retry_limit) + " retries")
              << utils::RESET << std::endl;
  }
  std::cout << std::endl;
  if (!network_ready) {
    utils::print_info("Check status with: exv status");
    utils::print_info("Check logs with: exv logs");
  }
  utils::print_info("Stop with: exv stop");
  return true;
}

bool stop_via_helper() {
  nlohmann::json response;
  std::string error_message;
  if (!send_request(nlohmann::json{{"action", "stop"}}, &response,
                    &error_message)) {
    utils::print_error(error_message);
    return false;
  }

  if (!response.value("ok", false)) {
    utils::print_error(response.value("message",
                                      std::string("Failed to stop VPN connection.")));
    return false;
  }

  std::cout << std::endl;
  utils::print_success(
      response.value("message", std::string("VPN connection stopped successfully! ðŸŽ‰")));
  return true;
}

bool show_status_via_helper() {
  nlohmann::json response;
  std::string error_message;
  if (!send_request(nlohmann::json{{"action", "status"}}, &response,
                    &error_message)) {
    utils::print_error(error_message);
    return false;
  }

  if (!response.value("ok", false)) {
    utils::print_error(response.value("message",
                                      std::string("Failed to query EXV helper status.")));
    return false;
  }

  return print_running_status(response);
}

int install_service(const std::string &executable_path) {
  return platform::install_helper_service(executable_path,
                                          make_helper_service_manager_context());
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
  try {
    nlohmann::json request =
        nlohmann::json::parse(utils::read_file(request_path));
    Config cfg = request.at("config").get<Config>();
    std::string plaintext_password = request.at("password").get<std::string>();
    int retry_limit = request.value("retry_limit", 0);
    uid_t uid = static_cast<uid_t>(request.at("uid").get<unsigned int>());
    gid_t gid = static_cast<gid_t>(request.at("gid").get<unsigned int>());
    std::string home = request.at("home").get<std::string>();
    std::string config_dir = request.at("config_dir").get<std::string>();

    runtime::bootstrap(config_dir, home, /*force=*/true);
    utils::set_runtime_owner(uid, gid);
    logger::init();
    int result = vpn::start_with_password(cfg, plaintext_password, retry_limit);
    utils::clear_runtime_owner();
    utils::clear_runtime_path_override();
    return result;
  } catch (...) {
    return 1;
  }
}

int daemon_main(const DaemonOptions &options) {
  active_daemon_options = options;
  if (active_daemon_options.endpoint.empty()) {
    active_daemon_options.endpoint = platform::helper_platform_config().endpoint;
  }
  if (active_daemon_options.mode.empty()) {
    active_daemon_options.mode =
        active_daemon_options.oneshot ? "oneshot" : "service";
  }

  signal(SIGTERM, daemon_signal_handler);
  signal(SIGINT, daemon_signal_handler);
  platform::setup_daemon_signals();

  auto ipc = create_ipc_server();

  platform::cleanup_daemon_endpoint(active_daemon_options.endpoint);

  if (!ipc->start(active_daemon_options.endpoint)) {
    return 1;
  }

  // V2 handler for structured privileged control-plane API.
  exv::helper::HelperV2Handler v2_handler;

  while (!daemon_stop_requested) {
    reap_finished_request_handlers();

    // Periodic V2 maintenance (session timeouts, cleanup).
    v2_handler.tick();

    if (!ipc->accept_client()) {
      if (daemon_stop_requested)
        break;
      continue;
    }

    if (!ipc->verify_client()) {
      ipc->close_client();
      continue;
    }

    std::string raw = ipc->read_request();
    unsigned int peer_uid = ipc->peer_uid();
    unsigned int peer_gid = ipc->peer_gid();

    // Route V2 vs V1 requests.  V2 requests carry an "op" field (numeric
    // HelperOp enum), while V1 requests carry an "action" string field.
    platform::dispatch_request_background(
        *ipc, raw, peer_uid, peer_gid,
        [&v2_handler](unsigned int uid, unsigned int gid,
           const nlohmann::json &req) -> nlohmann::json {
          // V2 protocol detection: "op" field present and numeric.
          if (req.contains("op") && req["op"].is_number_unsigned()) {
            try {
              exv::helper::HelperRequest v2_req =
                  exv::helper::helper_request_from_json(req);
              exv::helper::HelperResponse v2_resp = v2_handler.handle(v2_req);
              nlohmann::json out;
              exv::helper::to_json(out, v2_resp);
              return out;
            } catch (const std::exception &e) {
              return nlohmann::json{{"ok", false},
                                    {"code", "v2_protocol_error"},
                                    {"message", std::string("V2 handler error: ") + e.what()}};
            }
          }
          // V1 legacy dispatch.
          return handle_request(uid, gid, req);
        });

    if (active_daemon_options.oneshot) {
      try {
        nlohmann::json req = nlohmann::json::parse(raw);
        if (req.value("action", std::string()) == "stop") {
          daemon_stop_requested = 1;
        }
      } catch (...) {
      }
    }
  }

  reap_finished_request_handlers();

  SessionState state;
  if (load_session_state(&state)) {
    std::string message;
    stop_managed_session(state, &message);
    clear_session_state();
  }

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
