// Contract test: verifies that the TunnelStatusSnapshot -> frontend JSON
// mapping produces the fields and types that the WebUI expects.
//
// This test replicates the mapping logic from the desktop app API
// implementation to serve as a regression guard for the JSON contract.

#include "core/tunnel_controller/tunnel_state.hpp"
#include "core/tunnel_controller/tunnel_intent.hpp"

#include <nlohmann/json.hpp>

#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <string>

using json = nlohmann::json;

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;
  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

// Replicate the mapping from frontend_status_from_controller_snapshot.
// If this test breaks, the desktop status presenter must be updated to match.
json map_snapshot_to_frontend(const exv::core::TunnelStatusSnapshot &snap,
                              const std::string &server,
                              const std::string &username, int mtu,
                              int route_count) {
  bool running = snap.phase != exv::core::TunnelPhase::Idle &&
                 snap.phase != exv::core::TunnelPhase::Failed;
  bool connected = running && snap.network_ready;

  json j;
  j["connected"] = connected;
  j["process_running"] = running;
  j["server"] = snap.server.empty() ? server : snap.server;
  j["username"] = username;
  j["pid"] = -1;
  j["supervisor_pid"] = -1;
  j["network_ready"] = snap.network_ready;
  j["interface"] = snap.interface_name;
  j["internal_ip"] = "";
  j["route_count"] = route_count;
  j["mtu"] = mtu;
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
  return j;
}

std::string read_source_file(const std::filesystem::path &path) {
  std::ifstream in(path);
  return std::string((std::istreambuf_iterator<char>(in)),
                     std::istreambuf_iterator<char>());
}

std::string app_api_source_text() {
#ifndef ECNUVPN_SOURCE_DIR
  return std::string();
#else
  const auto app_api_dir = std::filesystem::path(ECNUVPN_SOURCE_DIR) / "src" /
                           "core" / "app_api";
  const char *files[] = {"app_api.cpp", "desktop_action_registry.cpp"};
  std::string source;
  for (const char *file : files) {
    source += "\n// ---- ";
    source += file;
    source += " ----\n";
    source += read_source_file(app_api_dir / file);
  }
  return source;
#endif
}

// -----------------------------------------------------------------------
// Test cases
// -----------------------------------------------------------------------

bool idle_snapshot_maps_to_disconnected() {
  exv::core::TunnelStatusSnapshot snap;
  snap.phase = exv::core::TunnelPhase::Idle;
  snap.network_ready = false;
  snap.server = "https://vpn.example.invalid";
  snap.interface_name = "";
  snap.auto_reconnect = true;

  json j = map_snapshot_to_frontend(snap, "https://vpn.example.invalid",
                                    "student", 1290, 9);

  bool ok = true;
  ok = expect(j.value("connected", true) == false,
              "idle phase should map to connected=false") &&
       ok;
  ok = expect(j.value("process_running", true) == false,
              "idle phase should map to process_running=false") &&
       ok;
  ok = expect(j.value("network_ready", true) == false,
              "idle phase should map to network_ready=false") &&
       ok;
  ok = expect(j.value("phase", std::string()) == "idle",
              "idle phase string should be 'idle'") &&
       ok;
  ok = expect(j.value("server", std::string()) ==
                  "https://vpn.example.invalid",
              "server should be preserved") &&
       ok;
  ok = expect(j.value("username", std::string()) == "student",
              "username should be preserved") &&
       ok;
  ok = expect(j.value("mtu", 0) == 1290, "mtu should be preserved") && ok;
  ok = expect(j.value("route_count", 0) == 9,
              "route_count should be preserved") &&
       ok;
  ok = expect(j.value("auto_reconnect", false) == true,
              "auto_reconnect should be preserved") &&
       ok;
  ok = expect(!j.contains("error"), "idle snapshot should not have error") &&
       ok;
  return ok;
}

bool connected_snapshot_maps_correctly() {
  exv::core::TunnelStatusSnapshot snap;
  snap.phase = exv::core::TunnelPhase::Connected;
  snap.network_ready = true;
  snap.server = "https://vpn.example.invalid";
  snap.interface_name = "ECNU-VPN";
  snap.auto_reconnect = true;

  json j = map_snapshot_to_frontend(snap, "https://vpn.example.invalid",
                                    "student", 1290, 9);

  bool ok = true;
  ok = expect(j.value("connected", false) == true,
              "connected phase should map to connected=true") &&
       ok;
  ok = expect(j.value("process_running", false) == true,
              "connected phase should map to process_running=true") &&
       ok;
  ok = expect(j.value("network_ready", false) == true,
              "connected phase should map to network_ready=true") &&
       ok;
  ok = expect(j.value("interface", std::string()) == "ECNU-VPN",
              "interface should be preserved") &&
       ok;
  ok = expect(j.value("phase", std::string()) == "connected",
              "phase string should be 'connected'") &&
       ok;
  return ok;
}

bool failed_snapshot_with_error_maps_correctly() {
  exv::core::TunnelStatusSnapshot snap;
  snap.phase = exv::core::TunnelPhase::Failed;
  snap.network_ready = false;
  snap.server = "https://vpn.example.invalid";
  snap.interface_name = "";

  exv::core::ErrorInfo err;
  err.domain = "auth";
  err.code = "auth_failed";
  err.message = "Authentication failed";
  err.recoverable = true;
  err.recommended_action = "Check credentials";
  snap.last_error = err;

  json j = map_snapshot_to_frontend(snap, "https://vpn.example.invalid",
                                    "student", 1290, 9);

  bool ok = true;
  ok = expect(j.value("connected", true) == false,
              "failed phase should map to connected=false") &&
       ok;
  ok = expect(j.value("process_running", true) == false,
              "failed phase should map to process_running=false") &&
       ok;
  ok = expect(j.value("phase", std::string()) == "failed",
              "phase string should be 'failed'") &&
       ok;
  ok = expect(j.value("error", std::string()) == "Authentication failed",
              "error message should be preserved") &&
       ok;
  ok = expect(j.value("error_code", std::string()) == "auth_failed",
              "error code should be preserved") &&
       ok;
  ok = expect(j.value("error_recoverable", false) == true,
              "error recoverable flag should be preserved") &&
       ok;
  return ok;
}

bool reconnecting_snapshot_maps_correctly() {
  exv::core::TunnelStatusSnapshot snap;
  snap.phase = exv::core::TunnelPhase::Reconnecting;
  snap.network_ready = false;
  snap.server = "https://vpn.example.invalid";
  snap.interface_name = "ECNU-VPN";
  snap.auto_reconnect = true;

  exv::core::ReconnectInfo ri;
  ri.attempt = 3;
  ri.next_retry_ms = 5000;
  snap.reconnect = ri;

  json j = map_snapshot_to_frontend(snap, "https://vpn.example.invalid",
                                    "student", 1290, 9);

  bool ok = true;
  ok = expect(j.value("connected", true) == false,
              "reconnecting should map to connected=false") &&
       ok;
  ok = expect(j.value("process_running", false) == true,
              "reconnecting should map to process_running=true") &&
       ok;
  ok = expect(j.value("phase", std::string()) == "reconnecting",
              "phase string should be 'reconnecting'") &&
       ok;
  ok = expect(j.value("reconnect_attempt", 0) == 3,
              "reconnect attempt should be preserved") &&
       ok;
  ok = expect(j.value("reconnect_next_retry_ms", 0) == 5000,
              "reconnect next_retry_ms should be preserved") &&
       ok;
  return ok;
}

bool all_phases_map_to_valid_strings() {
  const exv::core::TunnelPhase phases[] = {
      exv::core::TunnelPhase::Idle,
      exv::core::TunnelPhase::PreparingHelper,
      exv::core::TunnelPhase::Authenticating,
      exv::core::TunnelPhase::ConnectingCstp,
      exv::core::TunnelPhase::ApplyingNetworkConfig,
      exv::core::TunnelPhase::OpeningPacketDevice,
      exv::core::TunnelPhase::Connected,
      exv::core::TunnelPhase::Reconnecting,
      exv::core::TunnelPhase::Disconnecting,
      exv::core::TunnelPhase::CleaningUp,
      exv::core::TunnelPhase::Failed,
  };

  bool ok = true;
  for (auto phase : phases) {
    exv::core::TunnelStatusSnapshot snap;
    snap.phase = phase;

    json j = map_snapshot_to_frontend(snap, "server", "user", 1290, 0);
    std::string phase_str = j.value("phase", std::string());

    ok = expect(!phase_str.empty() && phase_str != "unknown",
                "every phase should map to a non-empty known string") &&
         ok;
  }
  return ok;
}

bool frontend_json_has_required_fields() {
  // Verify the JSON contract: all fields the WebUI relies on must be present.
  exv::core::TunnelStatusSnapshot snap;
  snap.phase = exv::core::TunnelPhase::Idle;

  json j = map_snapshot_to_frontend(snap, "server", "user", 1290, 0);

  const char *required_fields[] = {
      "connected",    "process_running", "server",     "username",
      "pid",          "supervisor_pid",  "network_ready",
      "interface",    "internal_ip",     "route_count",
      "mtu",          "uptime_seconds",  "rx_bytes",   "tx_bytes",
      "auto_reconnect", "phase",
  };

  bool ok = true;
  for (const auto &field : required_fields) {
    ok = expect(j.contains(field),
                (std::string("JSON must contain field: ") + field).c_str()) &&
         ok;
  }
  return ok;
}

bool app_api_activates_core_owned_native_mode() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source = app_api_source_text();

  const auto create_controller = source.find(
      "h.controller = std::make_shared<exv::core::TunnelController>");
  const auto mark_active =
      source.find("exv::core::set_tunnel_controller_active(true)",
                  create_controller == std::string::npos ? 0 : create_controller);
  const auto return_controller = source.find("return h.controller", mark_active);

  bool ok = true;
  ok = expect(create_controller != std::string::npos,
              "app_api should create the TunnelController") &&
       ok;
  ok = expect(mark_active != std::string::npos,
              "app_api should mark TunnelController active after creation") &&
       ok;
  ok = expect(return_controller != std::string::npos,
              "app_api should return the active TunnelController") &&
       ok;
  if (create_controller != std::string::npos &&
      mark_active != std::string::npos && return_controller != std::string::npos) {
    ok = expect(create_controller < mark_active,
                "active mark should occur after controller creation") &&
         ok;
    ok = expect(mark_active < return_controller,
                "active mark should occur before returning controller") &&
         ok;
  }
  return ok;
#endif
}

bool desktop_native_connect_uses_core_owned_controller_pipeline() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source = app_api_source_text();
  const auto connect_handler = source.find("adapter.register_legacy_handler(\"vpn.connect\"");
  const auto disconnect_handler = source.find("adapter.register_legacy_handler(\"vpn.disconnect\"",
                                               connect_handler);
  const auto connect_source =
      connect_handler == std::string::npos
          ? std::string()
          : source.substr(connect_handler,
                          disconnect_handler == std::string::npos
                              ? std::string::npos
                              : disconnect_handler - connect_handler);

  bool ok = true;
  ok = expect(connect_handler != std::string::npos,
              "app_api should register vpn.connect handler") &&
       ok;
  ok = expect(connect_source.find("Config cfg = mgr.load()") !=
                  std::string::npos,
              "desktop vpn.connect should load persisted config") &&
       ok;
  ok = expect(connect_source.find("password = crypto::decrypt") !=
                  std::string::npos,
              "desktop vpn.connect should resolve stored password before preflight") &&
       ok;
  ok = expect(connect_source.find("preflight_connect(cfg, password)") !=
                  std::string::npos,
              "desktop vpn.connect should use preflight_connect as prerequisite gate") &&
       ok;
  ok = expect(source.find("platform::resolve_backend(options)") !=
                  std::string::npos,
              "preflight_connect should resolve helper backend") &&
       ok;
  ok = expect(connect_source.find("try_acquire(attempt_opts)") !=
                  std::string::npos,
              "desktop vpn.connect should acquire the connection-attempt guard") &&
       ok;
  ok = expect(connect_source.find("ensure_tunnel_controller(helper_endpoint)") !=
                  std::string::npos,
              "desktop vpn.connect should initialize controller with resolved helper endpoint") &&
       ok;
  ok = expect(connect_source.find("controller->set_vpn_config(cfg, password)") !=
                  std::string::npos,
              "desktop vpn.connect should pass config and password into TunnelController") &&
       ok;
  ok = expect(connect_source.find("controller->connect(intent)") !=
                  std::string::npos,
              "desktop vpn.connect should delegate connection to TunnelController") &&
       ok;
  ok = expect(connect_source.find("platform::try_connect_direct_fallback") ==
                  std::string::npos,
              "desktop vpn.connect should not bypass controller with direct fallback") &&
       ok;
  return ok;
#endif
}

} // namespace

int main() {
  bool ok = true;
  ok = idle_snapshot_maps_to_disconnected() && ok;
  ok = connected_snapshot_maps_correctly() && ok;
  ok = failed_snapshot_with_error_maps_correctly() && ok;
  ok = reconnecting_snapshot_maps_correctly() && ok;
  ok = all_phases_map_to_valid_strings() && ok;
  ok = frontend_json_has_required_fields() && ok;
  ok = app_api_activates_core_owned_native_mode() && ok;
  ok = desktop_native_connect_uses_core_owned_controller_pipeline() && ok;
  return ok ? 0 : 1;
}
