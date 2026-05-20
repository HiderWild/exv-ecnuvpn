#include "helper.hpp"
#include "helper_ipc.hpp"

#include "logger.hpp"
#include "platform/common/helper_client.hpp"
#include "platform/common/helper_lifecycle.hpp"
#include "platform/common/helper_platform.hpp"
#include "platform/common/helper_service_manager.hpp"
#include "utils.hpp"
#include "vpn.hpp"
#include "virtual_network.hpp"

#include <cerrno>
#include <cctype>
#include <csignal>
#include <cstring>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

#ifndef _WIN32
#include <sys/stat.h>
#endif


namespace ecnuvpn {
namespace helper {

namespace {

volatile sig_atomic_t daemon_stop_requested = 0;

struct SessionState {
  uid_t uid = static_cast<uid_t>(-1);
  gid_t gid = static_cast<gid_t>(-1);
  std::string username;
  std::string home;
  std::string config_dir;
  std::string server;
  int route_count = 0;
  int retry_limit = 0;
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
  remove_file_if_exists(route_ready_path_for(state));
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
#ifndef _WIN32
  chmod(platform_config.session_state_path, 0600);
#endif
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

RuntimeSnapshot inspect_runtime(const SessionState &state) {
  RuntimeSnapshot snapshot;
  if (state.config_dir.empty())
    return snapshot;

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

nlohmann::json make_error(const std::string &message) {
  return nlohmann::json{{"ok", false}, {"message", message}};
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
                          {"server", state.server},
                          {"route_count", state.route_count},
                          {"retry_limit", state.retry_limit},
                          {"owner_username", state.username},
                          {"interfaces_output", snapshot.interfaces_output}};
  virtual_network::add_status_fields(response, snapshot.interface_name);
  return response;
}

bool ensure_same_owner(const SessionState &state, uid_t peer_uid) {
  // Any local user who can reach the helper socket may manage the VPN session.
  // Any local user who can reach the helper socket may manage the VPN session.
  // Socket is mode 0660, group staff (gid 20) — access controlled at OS level.
  (void)state;
  (void)peer_uid;
  return true;
}

nlohmann::json handle_status(uid_t peer_uid) {
  SessionState state;
  if (!load_session_state(&state)) {
    return nlohmann::json{{"ok", true}, {"running", false}};
  }

  if (!ensure_same_owner(state, peer_uid)) {
    return make_error("VPN session belongs to another local user.");
  }

  RuntimeSnapshot snapshot = inspect_runtime(state);
  if (!snapshot.running) {
  platform::cleanup_routes();
    clear_runtime_state(state);
    clear_session_state();
    return nlohmann::json{{"ok", true}, {"running", false}};
  }

  return make_status_response(state, snapshot);
}

bool stop_managed_session(const SessionState &state, std::string *message) {
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

  // Clean up routes before killing openconnect — while the tunnel
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
    platform::terminate_process(pid);
  if (supervisor_pid > 0 && is_process_alive(supervisor_pid))
    platform::terminate_process(supervisor_pid);

  platform::sleep_ms(500);

  pid_t remaining_pid = find_openconnect_pid();
  if (remaining_pid > 0 && remaining_pid != pid) {
    platform::terminate_process(remaining_pid);
    platform::sleep_ms(500);
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
    *message = "VPN connection stopped successfully! 🎉";
  return true;
}

nlohmann::json handle_stop(uid_t peer_uid) {
  SessionState state;
  if (!load_session_state(&state)) {
    // No session state — still clean up any orphaned routes/supervisors
  platform::cleanup_routes();
  platform::kill_all_supervisors();
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

  // stop_managed_session failed — belt-and-suspenders cleanup
  platform::cleanup_routes();
  platform::kill_all_supervisors();
  RuntimeSnapshot snapshot = inspect_runtime(state);
  if (!snapshot.running)
    clear_session_state();
  return make_error(message);
}

nlohmann::json handle_start(uid_t peer_uid, gid_t peer_gid,
                            const nlohmann::json &request) {
  SessionState existing;
  if (load_session_state(&existing)) {
    RuntimeSnapshot current = inspect_runtime(existing);
    if (current.running) {
      if (!ensure_same_owner(existing, peer_uid)) {
        return make_error("VPN session belongs to another local user.");
      }
      return nlohmann::json{{"ok", false},
                            {"message", "VPN is already running."},
                            {"running", true},
                            {"pid", current.pid},
                            {"supervisor_pid", current.supervisor_pid}};
    }
    clear_runtime_state(existing);
    clear_session_state();
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
    return make_error("Invalid start request payload.");
  }

  SessionState state;
  state.uid = peer_uid;
  state.gid = peer_gid;
  state.username = utils::get_username_for_uid(peer_uid);
  state.home = requested_home.empty() ? utils::get_home_for_uid(peer_uid)
                                    : requested_home;
  state.config_dir =
      requested_config_dir.empty() ? utils::get_config_dir_for_uid(peer_uid)
                                   : requested_config_dir;
  state.server = cfg.server;
  state.route_count = static_cast<int>(cfg.routes.size());
  state.retry_limit = retry_limit;

  nlohmann::json worker_request{{"uid", static_cast<unsigned int>(state.uid)},
                                {"gid", static_cast<unsigned int>(state.gid)},
                                {"home", state.home},
                                {"config_dir", state.config_dir},
                                {"retry_limit", retry_limit},
                                {"password", plaintext_password},
                                {"config", cfg}};

  std::string request_path;
  if (!create_request_file(worker_request, &request_path)) {
    return make_error("Failed to prepare helper request file.");
  }

  std::string executable_path = utils::get_executable_path();
  int status = platform::spawn_worker_process(executable_path, request_path);
  if (status < 0) {
    remove_file_if_exists(request_path);
    return make_error("Failed to launch EXV helper worker.");
  }

  remove_file_if_exists(request_path);

  RuntimeSnapshot snapshot = inspect_runtime(state);
  if (status == 0 && snapshot.running) {
    save_session_state(state);
    if (snapshot.network_ready) {
      return make_status_response(state, snapshot, true,
                                  "VPN connected successfully!");
    }
    return make_status_response(
        state, snapshot, true,
        "VPN process started, but network routes are not ready yet.");
  }

  clear_runtime_state(state);
  clear_session_state();
  return make_error("Failed to establish the VPN connection. Check logs with: exv logs");
}

nlohmann::json handle_request(uid_t peer_uid, gid_t peer_gid,
                              const nlohmann::json &request) {
  std::string action = request.value("action", std::string());
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
    std::cout << utils::RED << utils::BOLD << "  ● VPN is NOT RUNNING"
              << utils::RESET << std::endl;
    std::cout << std::endl;
    return true;
  }

  std::cout << utils::GREEN << utils::BOLD << "  ● VPN is RUNNING"
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
  nlohmann::json response;
  std::string error_message;
  if (!send_request(nlohmann::json{{"action", "start"},
                                   {"config", cfg},
                                   {"password", plaintext_password},
                                   {"retry_limit", retry_limit},
                                   {"home", utils::get_effective_home()},
                                   {"config_dir", utils::get_config_dir()}},
                    &response, &error_message, 120)) {
    utils::print_error(error_message);
    return false;
  }

  if (!response.value("ok", false)) {
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
      response.value("message", std::string("VPN connection stopped successfully! 🎉")));
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

    utils::set_runtime_path_override(home, config_dir);
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

int daemon_main() {
  signal(SIGTERM, daemon_signal_handler);
  signal(SIGINT, daemon_signal_handler);
#ifndef _WIN32
  signal(SIGPIPE, SIG_IGN);
#endif

  auto ipc = create_ipc_server();
  const auto &platform_config = platform::helper_platform_config();

#ifndef _WIN32
  remove_file_if_exists(platform_config.endpoint);
#endif

  if (!ipc->start(platform_config.endpoint)) {
    return 1;
  }

  while (!daemon_stop_requested) {
    reap_finished_request_handlers();

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

    platform::dispatch_request_background(
        *ipc, raw, peer_uid, peer_gid,
        [](unsigned int uid, unsigned int gid,
           const nlohmann::json &req) -> nlohmann::json {
          return handle_request(uid, gid, req);
        });
  }

  reap_finished_request_handlers();

  SessionState state;
  if (load_session_state(&state)) {
    std::string message;
    stop_managed_session(state, &message);
    clear_session_state();
  }

  ipc->close();
#ifndef _WIN32
  remove_file_if_exists(platform_config.endpoint);
#endif
  return 0;
}

} // namespace helper
} // namespace ecnuvpn
