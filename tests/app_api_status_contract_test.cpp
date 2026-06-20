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
#include <vector>

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
  const char *files[] = {"app_api.cpp", "desktop_action_registry.cpp",
                         "desktop_tunnel_host.cpp",
                         "desktop_vpn_actions.cpp",
                         "desktop_system_actions.cpp",
                         "desktop_status_presenter.cpp"};
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

std::string source_text_at(std::initializer_list<const char *> parts) {
#ifndef ECNUVPN_SOURCE_DIR
  return std::string();
#else
  std::filesystem::path path = std::filesystem::path(ECNUVPN_SOURCE_DIR);
  for (const char *part : parts) {
    path /= part;
  }
  return read_source_file(path);
#endif
}

std::vector<std::string> removed_public_runtime_fields() {
  return {
      std::string("openconnect") + "Binary",
      std::string("openconnect") + "Path",
      std::string("openconnect") + "Args",
      std::string("legacy") + "TunnelScript",
      std::string("legacy") + "Adapter",
  };
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
      "pid",          "network_ready",
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
  for (const auto &field : removed_public_runtime_fields()) {
    ok = expect(!j.contains(field),
                "frontend status JSON must not expose retired runtime fields") &&
         ok;
  }
  return ok;
}

bool disconnected_status_uses_async_virtual_network_probe_contract() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source =
      source_text_at({"src", "core", "app_api", "desktop_status_presenter.cpp"});
  const auto fn = source.find("nlohmann::json disconnected_status(");
  const auto next_fn =
      source.find("nlohmann::json frontend_status_from_snapshot_json", fn);
  const std::string disconnected_source =
      (fn == std::string::npos || next_fn == std::string::npos)
          ? std::string()
          : source.substr(fn, next_fn - fn);

  bool ok = true;
  ok = expect(fn != std::string::npos,
              "disconnected_status function should exist") &&
       ok;
  ok = expect(disconnected_source.find("add_cached_virtual_network_fields") !=
                  std::string::npos,
              "disconnected status.get should use cached virtual network fields and request async probing") &&
       ok;
  ok = expect(disconnected_source.find("virtual_network::add_status_fields") ==
                  std::string::npos,
              "disconnected status.get must not run platform virtual network probes synchronously") &&
       ok;
  return ok;
#endif
}

bool core_process_pushes_async_virtual_network_status_events_contract() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source =
      source_text_at({"src", "core", "core_process.cpp"});

  bool ok = true;
  ok = expect(source.find("drain_virtual_network_status_events") !=
                  std::string::npos,
              "core process should drain async virtual network status patches") &&
       ok;
  ok = expect(source.find("std::chrono::milliseconds(200)") !=
                  std::string::npos,
              "core process should check for async virtual network patches every 200ms") &&
       ok;
  ok = expect(source.find("\"event\", \"status\"") != std::string::npos ||
                  source.find("{\"event\", \"status\"}") != std::string::npos,
              "core process should send async virtual network patches as status events") &&
       ok;
  ok = expect(source.find("if (events.empty())") != std::string::npos,
              "core process should not emit empty async status responses") &&
       ok;
  return ok;
#endif
}

bool app_api_activates_core_owned_native_mode() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source = app_api_source_text();

  const auto create_controller =
      source.find("std::make_shared<exv::core::TunnelController>");
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
  const auto connect_handler = source.find("\"vpn.connect\"");
  const auto disconnect_handler = source.find("\"vpn.disconnect\"",
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
  ok = expect(connect_source.find("submit_connect") != std::string::npos,
              "desktop vpn.connect should submit an accepted connect job") &&
       ok;
  ok = expect(connect_source.find("return connect_state_json(state)") !=
                  std::string::npos,
              "desktop vpn.connect should return accepted job state promptly") &&
       ok;
  ok = expect(source.find("platform::resolve_backend(options)") !=
                  std::string::npos,
              "preflight_connect should resolve helper backend") &&
       ok;
  ok = expect(connect_source.find("try_acquire(attempt_opts)") !=
                  std::string::npos,
              "desktop vpn.connect should acquire the connection-attempt guard before accepted response") &&
       ok;
  ok = expect(source.find("exv::core::ConnectPipeline") !=
                  std::string::npos,
              "desktop connect job should coordinate backend/platform/protocol branches through ConnectPipeline") &&
       ok;
  ok = expect(source.find("ecnuvpn::vpn_engine::NativeHandshakeJob") !=
                  std::string::npos,
              "desktop connect job should prepare native protocol handshake before controller serial tail") &&
       ok;
  ok = expect(source.find("controller->set_prepared_native_handshake") !=
                  std::string::npos,
              "desktop connect job should pass prepared handshake into TunnelController") &&
       ok;
  ok = expect(source.find("ensure_tunnel_controller(helper_endpoint)") !=
                  std::string::npos,
              "desktop connect job should initialize controller with resolved helper endpoint") &&
       ok;
  ok = expect(source.find("controller->set_vpn_config(cfg, password)") !=
                  std::string::npos,
              "desktop connect job should pass config and password into TunnelController") &&
       ok;
  ok = expect(source.find("controller->connect(intent)") !=
                  std::string::npos,
              "desktop connect job should delegate connection to TunnelController") &&
       ok;
  ok = expect(connect_source.find("platform::try_connect_direct_fallback") ==
                  std::string::npos,
              "desktop vpn.connect should not bypass controller with direct fallback") &&
       ok;
  return ok;
#endif
}

bool desktop_native_preflight_reports_substage_timing() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source = app_api_source_text();
  const auto preflight_start = source.find("nlohmann::json preflight_connect");
  const auto preflight_end =
      source.find("void register_desktop_vpn_actions", preflight_start);
  const std::string preflight_source =
      preflight_start == std::string::npos
          ? std::string()
          : source.substr(preflight_start,
                          preflight_end == std::string::npos
                              ? std::string::npos
                              : preflight_end - preflight_start);

  bool ok = true;
  ok = expect(preflight_start != std::string::npos,
              "desktop app API should keep preflight_connect") &&
       ok;
  ok = expect(preflight_source.find("StageTimer timing(\"desktop.preflight_connect\")") !=
                  std::string::npos,
              "preflight_connect should own a substage timing scope") &&
       ok;
  ok = expect(preflight_source.find("timing.mark(\"config_validated\"") !=
                  std::string::npos,
              "preflight_connect should mark config validation separately") &&
       ok;
  ok = expect(preflight_source.find("timing.mark(\"backend_resolve_started\"") !=
                  std::string::npos,
              "preflight_connect should mark backend resolution start") &&
       ok;
  ok = expect(preflight_source.find("timing.mark(\"backend_resolved\"") !=
                  std::string::npos,
              "preflight_connect should mark backend resolution completion") &&
       ok;
  ok = expect(preflight_source.find("timing.mark(\"runtime_status_checked\"") !=
                  std::string::npos,
              "preflight_connect should mark runtime status separately") &&
       ok;
  ok = expect(preflight_source.find("timing.mark(\"platform_checks_checked\"") !=
                  std::string::npos,
              "preflight_connect should mark platform checks separately") &&
       ok;
  ok = expect(preflight_source.find("timing.finish(true, \"stage=preflight_complete\"") !=
                  std::string::npos,
              "preflight_connect should finish successful preflight timing") &&
       ok;
  return ok;
#endif
}

bool desktop_connect_pipeline_reports_branch_timing() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source = app_api_source_text();
  bool ok = true;
  ok = expect(source.find("StageTimer branch_timing(\"desktop.connect.backend_helper_ready\")") !=
                  std::string::npos,
              "desktop connect pipeline should time backend/helper readiness branch") &&
       ok;
  ok = expect(source.find("StageTimer branch_timing(\"desktop.connect.platform_ready\")") !=
                  std::string::npos,
              "desktop connect pipeline should time platform readiness branch") &&
       ok;
  ok = expect(source.find("StageTimer branch_timing(\"desktop.connect.protocol_handshake\")") !=
                  std::string::npos,
              "desktop connect pipeline should time protocol handshake branch") &&
       ok;
  ok = expect(source.find("timing.mark(\"first_failure\"") !=
                  std::string::npos,
              "desktop connect pipeline should mark first failure timing") &&
       ok;
  ok = expect(source.find("timing.mark(\"serial_tail\"") !=
                  std::string::npos,
              "desktop connect pipeline should mark serial tail entry") &&
       ok;
  ok = expect(source.find("timing.mark(\"background_job_started\"") !=
                  std::string::npos,
              "desktop connect background job should log before doing work") &&
       ok;
  ok = expect(source.find("branch_timing.mark(\"backend_resolve_started\"") !=
                  std::string::npos,
              "desktop connect should log before backend resolution") &&
       ok;
  ok = expect(source.find("branch_timing.mark(\"runtime_status_check_started\"") !=
                  std::string::npos,
              "desktop connect should log before runtime status checks") &&
       ok;
  ok = expect(source.find("branch_timing.mark(\"native_config_mapping_started\"") !=
                  std::string::npos,
              "desktop connect should log before native config mapping") &&
       ok;
  ok = expect(source.find("branch_timing.mark(\"native_handshake_started\"") !=
                  std::string::npos,
              "desktop connect should log before native handshake") &&
       ok;
  ok = expect(source.find("timing.mark(\"tunnel_controller_connect_start\"") !=
                  std::string::npos,
              "desktop connect should log before invoking TunnelController::connect") &&
       ok;
  // Prepared-handshake event sink: the protocol_branch must wire
  // NativeHandshakeJob's event_sink to a real log-emitting sink, otherwise
  // auth.started / auth.failed / cstp.failed events from the most common
  // failure layer of a real connect attempt are silently dropped. We reuse
  // EngineEventBridge as a log-only sink (nullptr TunnelEvent callback) so
  // the prepared-handshake path emits the same engine.* log line shape as
  // the runner-driven path. See
  // docs/AGGREGATE_AUTH_EMPTY_RESPONSE_FIX_PLAN.md §5.
  ok = expect(source.find("EngineEventBridge") != std::string::npos,
              "desktop connect protocol branch should reuse EngineEventBridge "
              "for log-only engine event redaction") &&
       ok;
  {
    const auto protocol_branch_start =
        source.find("StageTimer branch_timing(\"desktop.connect.protocol_handshake\")");
    const auto protocol_branch_end =
        protocol_branch_start == std::string::npos
            ? std::string::npos
            : source.find("auto pipeline_result", protocol_branch_start);
    const std::string protocol_branch_source =
        (protocol_branch_start == std::string::npos ||
         protocol_branch_end == std::string::npos)
            ? std::string()
            : source.substr(protocol_branch_start,
                            protocol_branch_end - protocol_branch_start);
    ok = expect(!protocol_branch_source.empty(),
                "protocol_handshake branch source slice should be locatable") &&
         ok;
    ok = expect(protocol_branch_source.find("EngineEventBridge") !=
                    std::string::npos,
                "protocol_branch should construct EngineEventBridge before "
                "running NativeHandshakeJob") &&
         ok;
    ok = expect(protocol_branch_source.find("deps.event_sink =") !=
                    std::string::npos,
                "protocol_branch should assign deps.event_sink so handshake "
                "events are not dropped") &&
         ok;
    // Auth-interaction coordinator: aggregate-auth may demand group_select
    // or challenge during the prepared handshake, before the runner exists.
    // The connect job must publish a coordinator into the global slot so
    // vpn.authInteraction.get / respond can drive the prompt to completion.
    // See docs/AGGREGATE_AUTH_EMPTY_RESPONSE_FIX_PLAN.md §6.
    ok = expect(protocol_branch_source.find("AuthInteractionCoordinator") !=
                    std::string::npos,
                "protocol_branch should construct AuthInteractionCoordinator "
                "for prepared-handshake group/challenge prompts") &&
         ok;
    ok = expect(protocol_branch_source.find("set_active_connect_auth_coordinator") !=
                    std::string::npos,
                "protocol_branch should publish the coordinator into the "
                "active-connect global slot") &&
         ok;
    ok = expect(protocol_branch_source.find(
                    "clear_active_connect_auth_coordinator_if_current") !=
                    std::string::npos,
                "protocol_branch should clear the active-connect coordinator "
                "with a compare-and-clear guard so stale detached branches "
                "cannot erase newer connect prompts") &&
         ok;
    ok = expect(protocol_branch_source.find("deps.auth_interaction_handler =") !=
                    std::string::npos,
                "protocol_branch should wire deps.auth_interaction_handler so "
                "the handshake job can route prompts to the coordinator") &&
         ok;
  }
  // RPC handlers must consult the connect-job coordinator before the
  // controller — during prepared-handshake the controller has no pending
  // interaction, so the controller-only path would always say "no pending"
  // and the user could not answer a group_select / challenge prompt.
  ok = expect(source.find("get_active_connect_auth_coordinator") !=
                  std::string::npos,
              "vpn.authInteraction.get/respond must consult the connect-job "
              "coordinator before the controller") &&
       ok;
  return ok;
#endif
}

bool desktop_connect_pipeline_tracks_and_reaps_unused_oneshot_helper() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source = app_api_source_text();
  const auto background_job = source.find("void run_desktop_connect_job");
  const auto pipeline_failure = source.find("if (!pipeline_result.ok)",
                                            background_job);
  const auto failure_error = source.find("set_desktop_connect_error",
                                         pipeline_failure);
  const auto helper_pid_update =
      source.find("conn_attempt::update_pids_if_current", background_job);
  const auto helper_cleanup =
      source.find("cleanup_unused_oneshot_backend", pipeline_failure);
  const auto controller_init =
      source.find("auto controller = ensure_tunnel_controller(helper_endpoint);",
                  background_job);
  const auto controller_config =
      source.find("controller->set_vpn_config(cfg, password)", controller_init);
  const std::string controller_init_to_config =
      controller_init == std::string::npos
          ? std::string()
          : source.substr(controller_init,
                          controller_config == std::string::npos
                              ? std::string::npos
                              : controller_config - controller_init);
  const auto moved_attempt_id = source.find("std::move(attempt_id)",
                                            background_job);
  const auto backend_branch =
      source.find("StageTimer branch_timing(\"desktop.connect.backend_helper_ready\")",
                  background_job);
  const auto backend_branch_end =
      source.find("StageTimer branch_timing(\"desktop.connect.platform_ready\")",
                  backend_branch);
  const std::string backend_source =
      backend_branch == std::string::npos
          ? std::string()
          : source.substr(backend_branch,
                          backend_branch_end == std::string::npos
                              ? std::string::npos
                              : backend_branch_end - backend_branch);

  bool ok = true;
  ok = expect(background_job != std::string::npos,
              "desktop connect job should run heavy work in background") &&
       ok;
  ok = expect(helper_pid_update != std::string::npos,
              "desktop connect backend branch should persist the started oneshot helper pid into the active attempt record") &&
       ok;
  ok = expect(moved_attempt_id == std::string::npos,
              "desktop connect job must not move attempt_id before backend branch records helper pid") &&
       ok;
  ok = expect(backend_source.find("branch_stop.stop_requested()") !=
                  std::string::npos &&
                  backend_source.find("cleanup_unused_oneshot_backend(backend)") !=
                      std::string::npos,
              "backend branch should cleanup its own started oneshot helper if another branch already cancelled the pipeline") &&
       ok;
  ok = expect(pipeline_failure != std::string::npos,
              "desktop connect job should handle first-failure pipeline result") &&
       ok;
  ok = expect(helper_cleanup != std::string::npos,
              "desktop connect pipeline failure should cleanup an unused oneshot helper before returning") &&
       ok;
  if (pipeline_failure != std::string::npos &&
      helper_cleanup != std::string::npos && failure_error != std::string::npos) {
    ok = expect(pipeline_failure < helper_cleanup &&
                    helper_cleanup < failure_error,
                "unused oneshot helper cleanup should run before surfacing pipeline failure") &&
         ok;
  }
  ok = expect(controller_init_to_config.find("if (stop.stop_requested())") ==
                  std::string::npos ||
                  controller_init_to_config.find("reset_tunnel_controller()") !=
                      std::string::npos,
              "stop after controller initialization should reset the unused controller/helper before returning") &&
       ok;
  return ok;
#endif
}

bool desktop_native_connect_releases_attempt_guard_on_failed_status() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source = app_api_source_text();
  const auto connect_handler = source.find("\"vpn.connect\"");
  const auto disconnect_handler = source.find("\"vpn.disconnect\"",
                                               connect_handler);
  const auto connect_source =
      connect_handler == std::string::npos
          ? std::string()
          : source.substr(connect_handler,
                          disconnect_handler == std::string::npos
                              ? std::string::npos
                              : disconnect_handler - connect_handler);

  const auto accepted_return =
      connect_source.find("return connect_state_json(state)");
  const auto synchronous_status =
      connect_source.find("auto snap = controller->status()");
  const auto synchronous_failed_status =
      connect_source.find("snap.phase == exv::core::TunnelPhase::Failed");
  const auto background_job =
      source.find("void run_desktop_connect_job");
  const auto failed_cleanup =
      source.find("reset_tunnel_controller()", background_job);
  const auto dismiss_attempt =
      source.find("attempt_cleanup.dismiss()", background_job);

  bool ok = true;
  ok = expect(connect_handler != std::string::npos,
              "app_api should register vpn.connect handler") &&
       ok;
  ok = expect(accepted_return != std::string::npos,
              "desktop vpn.connect should return accepted job state") &&
       ok;
  ok = expect(synchronous_status == std::string::npos,
              "desktop vpn.connect handler must not synchronously inspect controller status after accepted job submission") &&
       ok;
  ok = expect(synchronous_failed_status == std::string::npos,
              "desktop vpn.connect handler must not synchronously wait for failed controller status") &&
       ok;
  ok = expect(background_job != std::string::npos,
              "desktop vpn.connect should move heavy work into a background job") &&
       ok;
  ok = expect(failed_cleanup != std::string::npos,
              "desktop connect job should reset failed controller state in background") &&
       ok;
  ok = expect(dismiss_attempt != std::string::npos,
              "desktop connect job should dismiss the attempt guard only after non-terminal startup") &&
       ok;
  if (background_job != std::string::npos && failed_cleanup != std::string::npos &&
      dismiss_attempt != std::string::npos) {
    ok = expect(failed_cleanup < dismiss_attempt,
                "failed background connect must reset controller before dismissing attempt guard") &&
         ok;
  }
  return ok;
#endif
}

bool desktop_connect_error_preserves_status_error_code_contract() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source = app_api_source_text();
  const auto apply_error = source.find("void apply_desktop_connect_error");
  const auto apply_job = source.find("void apply_desktop_connect_job_status",
                                     apply_error);
  const auto block =
      apply_error == std::string::npos
          ? std::string()
          : source.substr(apply_error,
                          apply_job == std::string::npos
                              ? std::string::npos
                              : apply_job - apply_error);

  bool ok = true;
  ok = expect(apply_error != std::string::npos,
              "desktop status should merge stored connect failures") &&
       ok;
  ok = expect(block.find("json_string(*failure, \"error_code\"") !=
                  std::string::npos,
              "stored connect failure merge should read frontend status error_code") &&
       ok;
  ok = expect(block.find("json_string(*failure, \"code\"") !=
                  std::string::npos,
              "stored connect failure merge should still accept canonical code") &&
       ok;
  return ok;
#endif
}

bool tunnel_controller_native_runner_failure_cleans_helper_session() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source =
      source_text_at({"src", "core", "tunnel_controller",
                      "tunnel_controller_connect.cpp"});
  const auto runner_start = source.find("bool ok = false");
  const auto prepared_runner_start = source.find("runner_.start_from_handshake",
                                                 runner_start);
  const auto direct_runner_start = source.find("runner_.start(vpn_cfg_, vpn_password_)",
                                               runner_start);
  const auto runner_failed = source.find("if (!ok)", runner_start);
  const auto runner_started =
      source.find("\"native.runner.started\"", runner_failed);
  const auto failure_source =
      runner_failed == std::string::npos
          ? std::string()
          : source.substr(runner_failed,
                          runner_started == std::string::npos
                              ? std::string::npos
                              : runner_started - runner_failed);
  const auto session_gate = failure_source.find("!session_id_.value.empty()");
  const auto cleanup_call = failure_source.find("cleanup_after_failed_startup()");
  const auto failed_transition =
      failure_source.find("transition_to(TunnelPhase::Failed)");

  bool ok = true;
  ok = expect(runner_start != std::string::npos,
              "TunnelController should start the native runner in connect flow") &&
       ok;
  ok = expect(prepared_runner_start != std::string::npos,
              "TunnelController should support prepared native handshake runner start") &&
       ok;
  ok = expect(direct_runner_start != std::string::npos,
              "TunnelController should preserve direct native runner start fallback") &&
       ok;
  ok = expect(runner_failed != std::string::npos,
              "TunnelController should handle native runner start failure") &&
       ok;
  ok = expect(session_gate != std::string::npos,
              "native runner failure cleanup should run when a helper session was already started") &&
       ok;
  ok = expect(cleanup_call != std::string::npos,
              "native runner failure should shut down the helper session before returning failed status") &&
       ok;
  ok = expect(failed_transition != std::string::npos,
              "native runner failure should still transition to Failed") &&
       ok;
  if (session_gate != std::string::npos && cleanup_call != std::string::npos &&
      failed_transition != std::string::npos) {
    ok = expect(session_gate < cleanup_call,
                "helper-session gate should guard failed-startup cleanup") &&
         ok;
    ok = expect(cleanup_call < failed_transition,
                "helper session should be cleaned before Failed snapshot is emitted") &&
         ok;
  }
  return ok;
#endif
}

bool desktop_connect_job_owner_destructs_before_error_state_contract() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source = app_api_source_text();
  const auto error_mutex = source.find("g_desktop_connect_error_mutex");
  const auto error_state = source.find("g_desktop_connect_error", error_mutex);
  const auto jobs_mutex = source.find("g_desktop_connect_jobs_mutex");
  const auto jobs_owner = source.find("g_desktop_connect_jobs", jobs_mutex);

  bool ok = true;
  ok = expect(error_mutex != std::string::npos &&
                  error_state != std::string::npos &&
                  jobs_mutex != std::string::npos &&
                  jobs_owner != std::string::npos,
              "desktop connect globals should be present") &&
       ok;
  ok = expect(error_state < jobs_owner,
              "desktop connect job owner must be declared after error state so it destructs first and joins background work before error state is destroyed") &&
       ok;
  return ok;
#endif
}

bool desktop_helper_status_includes_current_instance_contract() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source = app_api_source_text();
  const auto helper_status_handler = source.find("\"helper.status\"");
  const auto runtime_status_handler = source.find("\"runtime.status\"",
                                                   helper_status_handler);
  const auto helper_status_source =
      helper_status_handler == std::string::npos
          ? std::string()
          : source.substr(helper_status_handler,
                          runtime_status_handler == std::string::npos
                              ? std::string::npos
                              : runtime_status_handler - helper_status_handler);

  bool ok = true;
  ok = expect(helper_status_handler != std::string::npos,
              "desktop API should register helper.status") &&
       ok;
  ok = expect(helper_status_source.find("helper_status_with_current_instance") !=
                  std::string::npos,
              "desktop helper.status should enrich backend status with current instance") &&
       ok;
  ok = expect(source.find("\"current_instance\"") != std::string::npos,
              "desktop helper.status response should include current_instance") &&
       ok;
  ok = expect(source.find("\"service_status\"") != std::string::npos,
              "desktop helper.status should preserve service_status field") &&
       ok;
  ok = expect(source.find("get_tunnel_controller_if_exists()") !=
                  std::string::npos,
              "desktop helper.status should inspect active TunnelController") &&
       ok;
  return ok;
#endif
}

bool auth_interaction_coordinator_test_is_release_blocking() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string cmake = source_text_at({"CMakeLists.txt"});
  const auto list_start = cmake.find("set(_release_blocking_tests");
  const auto list_end = list_start == std::string::npos
                            ? std::string::npos
                            : cmake.find("\n)", list_start);
  const std::string release_blocking =
      (list_start == std::string::npos || list_end == std::string::npos)
          ? std::string()
          : cmake.substr(list_start, list_end - list_start);

  bool ok = true;
  ok = expect(!release_blocking.empty(),
              "CMake release-blocking test list should be locatable") &&
       ok;
  ok = expect(release_blocking.find("auth_interaction_coordinator_test") !=
                  std::string::npos,
              "auth_interaction_coordinator_test must be in the CTest "
              "release-blocking list, not only target LABELS") &&
       ok;
  return ok;
#endif
}

bool desktop_service_actions_use_helper_owned_maintenance_contract() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string source = app_api_source_text();
  const auto service_install_handler = source.find("\"service.install\"");
  const auto service_uninstall_handler = source.find("\"service.uninstall\"");

  bool ok = true;
  ok = expect(service_install_handler != std::string::npos,
              "desktop API should register service.install") &&
       ok;
  ok = expect(service_uninstall_handler != std::string::npos,
              "desktop API should register service.uninstall") &&
       ok;
  ok = expect(source.find("get_current_helper_client_if_exists()") !=
                  std::string::npos,
              "desktop service actions should reuse the current helper client") &&
       ok;
  ok = expect(source.find("client->install_service") != std::string::npos,
              "desktop service.install should call helper InstallService") &&
       ok;
  ok = expect(source.find("export_cleanup_lease") != std::string::npos,
              "desktop service.install should export cleanup lease for oneshot handoff") &&
       ok;
  ok = expect(source.find("handoff_session") != std::string::npos,
              "desktop service.install should ask service helper to adopt cleanup lease") &&
       ok;
  ok = expect(source.find("replace_tunnel_controller_helper_for_handoff") !=
                  std::string::npos,
              "desktop service.install should switch controller to service helper") &&
       ok;
  ok = expect(source.find("finalize_handoff") != std::string::npos,
              "desktop service.install should finalize oneshot handoff") &&
       ok;
  ok = expect(source.find("client->uninstall_service") == std::string::npos,
              "desktop service.uninstall must not ask the running service helper to uninstall itself") &&
       ok;
  ok = expect(source.find("make_system_status_use_cases().uninstall_helper()") !=
                  std::string::npos,
              "desktop service.uninstall should delegate to the shared one-shot helper maintenance path") &&
       ok;
  ok = expect(source.find("\"vpn_session_active\"") != std::string::npos,
              "desktop service.uninstall should reject active VPN sessions") &&
       ok;
  return ok;
#endif
}

bool cli_service_actions_use_core_helper_maintenance_contract() {
#ifndef ECNUVPN_SOURCE_DIR
  std::cerr << "EXPECT FAILED: ECNUVPN_SOURCE_DIR is not defined" << std::endl;
  return false;
#else
  const std::string cli_source =
      source_text_at({"src", "cli", "cli_commands.cpp"});
  const std::string use_case_source =
      source_text_at({"src", "core", "use_cases",
                      "system_status_use_cases.cpp"});
  const auto service_handler = cli_source.find("\"service\"");
  const auto config_handler = cli_source.find("\"config\"",
                                               service_handler);

  bool ok = true;
  ok = expect(service_handler != std::string::npos,
              "CLI should keep a service subcommand handler") &&
       ok;
  ok = expect(cli_source.find("format_core_request") !=
                  std::string::npos,
              "CLI service install/uninstall should route through core action dispatch") &&
       ok;
  ok = expect(cli_source.find("helper::install_service") ==
                  std::string::npos,
              "CLI service install must not call helper::install_service directly") &&
       ok;
  ok = expect(cli_source.find("helper::uninstall_service") ==
                  std::string::npos,
              "CLI service uninstall must not call helper::uninstall_service directly") &&
       ok;
  ok = expect(use_case_source.find("start_oneshot = true") !=
                  std::string::npos,
              "first service.install should explicitly bootstrap a one-shot helper") &&
       ok;
  ok = expect(use_case_source.find("client->hello") <
                  use_case_source.find("client->acquire_core_lease"),
              "service maintenance should send Hello before acquiring the core lease") &&
       ok;
  ok = expect(use_case_source.find("client.install_service") !=
                  std::string::npos,
              "service.install should call helper InstallService through IPC") &&
       ok;
  ok = expect(use_case_source.find("client.uninstall_service") !=
                  std::string::npos,
              "service.uninstall should call helper UninstallService through IPC") &&
       ok;
  ok = expect(use_case_source.find(
                  "with_helper_service_lease(\"service.uninstall\", true") !=
                  std::string::npos,
              "service.uninstall should bootstrap a one-shot helper when the installed service is stopped") &&
       ok;
  ok = expect(use_case_source.find(
                  "with_helper_service_lease(\"service.uninstall\", true, \"oneshot\"") !=
                  std::string::npos,
              "service.uninstall must force oneshot backend instead of asking the running service to uninstall itself") &&
       ok;
  ok = expect(use_case_source.find("read_runtime_status_snapshot") !=
                  std::string::npos &&
                  use_case_source.find("\"vpn_session_active\"") !=
                      std::string::npos,
              "CLI/shared service.uninstall should reject active VPN runtime state") &&
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
  ok = disconnected_status_uses_async_virtual_network_probe_contract() && ok;
  ok = core_process_pushes_async_virtual_network_status_events_contract() && ok;
  ok = app_api_activates_core_owned_native_mode() && ok;
  ok = desktop_native_connect_uses_core_owned_controller_pipeline() && ok;
  ok = desktop_native_preflight_reports_substage_timing() && ok;
  ok = desktop_connect_pipeline_reports_branch_timing() && ok;
  ok = desktop_connect_pipeline_tracks_and_reaps_unused_oneshot_helper() && ok;
  ok = desktop_native_connect_releases_attempt_guard_on_failed_status() && ok;
  ok = desktop_connect_error_preserves_status_error_code_contract() && ok;
  ok = tunnel_controller_native_runner_failure_cleans_helper_session() && ok;
  ok = desktop_connect_job_owner_destructs_before_error_state_contract() && ok;
  ok = desktop_helper_status_includes_current_instance_contract() && ok;
  ok = auth_interaction_coordinator_test_is_release_blocking() && ok;
  ok = desktop_service_actions_use_helper_owned_maintenance_contract() && ok;
  ok = cli_service_actions_use_core_helper_maintenance_contract() && ok;
  return ok ? 0 : 1;
}
