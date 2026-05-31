#include "vpn_engine/native_error_contract.hpp"
#include "vpn_engine/native_session_store.hpp"
#include "vpn_engine/session_state.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <map>
#include <string>
#include <utility>
#include <vector>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif

namespace {

int current_process_id() {
#ifdef _WIN32
  return static_cast<int>(GetCurrentProcessId());
#else
  return static_cast<int>(getpid());
#endif
}

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

void write_text(const std::string &path, const std::string &content) {
  std::ofstream out(path, std::ios::binary);
  out << content;
}

ecnuvpn::vpn_engine::TunnelMetadata tunnel_metadata() {
  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  metadata.interface_name = "tun-native0";
  metadata.interface_index = 42;
  metadata.internal_ip4_address = "10.8.0.23";
  metadata.internal_ip4_netmask = "255.255.255.255";
  metadata.mtu = 1280;
  metadata.routes = {"59.78.176.0/20", "10.10.0.0/16"};
  metadata.server_bypass_ips = {"203.0.113.7"};
  return metadata;
}

ecnuvpn::vpn_engine::NativeSessionRecord ready_record() {
  ecnuvpn::vpn_engine::NativeSessionRecord record;
  record.session.tunnel_configured(tunnel_metadata());
  record.session.packet_loop_started();
  record.pid = current_process_id();
  record.supervisor_pid = -1;
  record.server = "vpn.example.edu";
  record.route_count = 2;
  record.retry_limit = 3;
  return record;
}

ecnuvpn::vpn_engine::NativeSessionProbe live_current_process_probe() {
  ecnuvpn::vpn_engine::NativeSessionProbe probe;
  probe.is_process_alive = [](int pid) { return pid == current_process_id(); };
  return probe;
}

ecnuvpn::vpn_engine::VpnEngineEvent
native_event(const std::string &type, const std::string &level,
             const std::string &message,
             std::map<std::string, std::string> fields = {}) {
  ecnuvpn::vpn_engine::VpnEngineEvent event;
  event.type = type;
  event.level = level;
  event.message = message;
  event.fields = std::move(fields);
  return event;
}

bool test_ready_state_uses_native_json_and_emits_compat_marker(
    const std::filesystem::path &root) {
  namespace store = ecnuvpn::vpn_engine;

  const std::string config_dir = root.string();
  store::clear_native_session_state(config_dir);

  bool ok = true;
  ok = expect(store::save_native_session_state(config_dir, ready_record()),
              "saving native ready state should succeed") &&
       ok;
  ok = expect(std::filesystem::exists(
                  store::native_session_state_path(config_dir)),
              "native-session-state.json should be created") &&
       ok;
  ok = expect(std::filesystem::exists(store::route_ready_path(config_dir)),
              "saving ready native state should emit route-ready marker") &&
       ok;

  store::NativeSessionSnapshot snapshot =
      store::read_native_session_snapshot(config_dir,
                                          live_current_process_probe());
  ok = expect(snapshot.running, "ready native snapshot should be running") && ok;
  ok = expect(snapshot.network_ready,
              "ready native snapshot should be network-ready") &&
       ok;
  ok = expect(snapshot.pid == current_process_id(),
              "native snapshot should use persisted native pid") &&
       ok;
  ok = expect(snapshot.interface_name == "tun-native0",
              "native snapshot should expose persisted interface") &&
       ok;
  ok = expect(snapshot.internal_ip == "10.8.0.23",
              "native snapshot should expose persisted internal IP") &&
       ok;
  ok = expect(snapshot.server == "vpn.example.edu",
              "native snapshot should expose persisted server") &&
       ok;
  ok = expect(snapshot.route_count == 2,
              "native snapshot should expose persisted route count") &&
       ok;
  ok = expect(snapshot.retry_limit == 3,
              "native snapshot should expose persisted retry limit") &&
       ok;

  std::filesystem::remove(store::route_ready_path(config_dir));
  store::NativeSessionSnapshot without_marker =
      store::read_native_session_snapshot(config_dir,
                                          live_current_process_probe());
  ok = expect(without_marker.running,
              "deleting route-ready marker should not break native status") &&
       ok;
  ok = expect(without_marker.network_ready,
              "native network readiness should come from persisted SessionState") &&
       ok;

  store::clear_native_session_state(config_dir);
  ok = expect(!std::filesystem::exists(
                  store::native_session_state_path(config_dir)),
              "forced clear should remove native-session-state.json") &&
       ok;

  return ok;
}

bool test_tunnel_configured_without_packet_loop_is_not_network_ready(
    const std::filesystem::path &root) {
  namespace store = ecnuvpn::vpn_engine;

  const std::string config_dir = root.string();
  store::clear_native_session_state(config_dir);

  store::NativeSessionRecord record;
  record.session.tunnel_configured(tunnel_metadata());
  record.pid = current_process_id();
  record.server = "vpn.example.edu";
  record.route_count = 2;

  bool ok = true;
  ok = expect(store::save_native_session_state(config_dir, record),
              "saving tunnel-configured native state should succeed") &&
       ok;
  write_text(store::route_ready_path(config_dir), "tun-native0\n10.8.0.23\n");

  store::NativeSessionSnapshot snapshot =
      store::read_native_session_snapshot(config_dir,
                                          live_current_process_probe());
  ok = expect(!snapshot.network_ready,
              "route-ready marker must not make native state network-ready before packet loop") &&
       ok;
  ok = expect(snapshot.running,
              "live tunnel-configured native state should be running before packet loop") &&
       ok;

  return ok;
}

bool test_missing_liveness_probe_does_not_trust_pid(
    const std::filesystem::path &root) {
  namespace store = ecnuvpn::vpn_engine;

  const std::string config_dir = root.string();
  store::clear_native_session_state(config_dir);

  store::NativeSessionRecord record = ready_record();
  record.pid = current_process_id();

  bool ok = true;
  ok = expect(store::save_native_session_state(config_dir, record),
              "saving native ready state should succeed") &&
       ok;

  store::NativeSessionSnapshot snapshot =
      store::read_native_session_snapshot(config_dir, store::NativeSessionProbe{});
  ok = expect(!snapshot.running,
              "missing liveness probe must not trust an arbitrary native pid") &&
       ok;
  ok = expect(!snapshot.network_ready,
              "missing liveness probe must not report native network readiness") &&
       ok;

  return ok;
}

bool test_event_recorder_persists_worker_session_events(
    const std::filesystem::path &root) {
  namespace store = ecnuvpn::vpn_engine;

  const std::string config_dir = (root / "event-recorder").string();
  store::clear_native_session_state(config_dir);

  store::NativeSessionRecord seed;
  seed.pid = current_process_id();
  seed.server = "vpn.example.edu";
  seed.route_count = 2;
  seed.retry_limit = 4;

  store::NativeSessionEventRecorder recorder(config_dir, seed);
  recorder.emit(native_event("auth.started", "info", "password auth started"));
  recorder.emit(native_event("auth.succeeded", "info", "password auth succeeded"));
  recorder.emit(native_event(
      "cstp.connected", "info", "CSTP connect succeeded",
      {{"interface", "tun-native0"}, {"internal_ip", "10.8.0.23"}}));

  bool ok = true;
  ok = expect(std::filesystem::exists(
                  store::native_session_state_path(config_dir)),
              "native worker event recorder should write native-session-state.json") &&
       ok;

  store::NativeSessionSnapshot configured =
      store::read_native_session_snapshot(config_dir,
                                          live_current_process_probe());
  ok = expect(configured.running,
              "native worker events should persist a live pre-packet-loop session") &&
       ok;
  ok = expect(!configured.network_ready,
              "native worker events must not fake network readiness before packet loop") &&
       ok;
  ok = expect(configured.interface_name == "tun-native0",
              "native worker events should persist tunnel interface metadata") &&
       ok;
  ok = expect(configured.internal_ip == "10.8.0.23",
              "native worker events should persist tunnel IP metadata") &&
       ok;
  ok = expect(configured.server == "vpn.example.edu",
              "native worker events should persist server metadata") &&
       ok;
  ok = expect(configured.retry_limit == 4,
              "native worker events should persist retry metadata") &&
       ok;

  recorder.emit(
      native_event("packet.loop.started", "info", "packet loop started"));
  store::NativeSessionSnapshot ready =
      store::read_native_session_snapshot(config_dir,
                                          live_current_process_probe());
  ok = expect(ready.running,
              "native packet-loop event should keep the persisted session running") &&
       ok;
  ok = expect(ready.network_ready,
              "native packet-loop event should mark the persisted session network-ready") &&
       ok;

  const std::string failed_dir = (root / "event-recorder-failed").string();
  store::clear_native_session_state(failed_dir);
  store::NativeSessionEventRecorder failed_recorder(failed_dir, seed);
  failed_recorder.emit(native_event("native.start.failed", "error",
                                    "native transport unavailable",
                                    {{"code", "native_transport_unimplemented"}}));
  store::NativeSessionSnapshot failed =
      store::read_native_session_snapshot(failed_dir,
                                          live_current_process_probe());
  ok = expect(!failed.running,
              "native failure event must not persist a false running session") &&
       ok;

  return ok;
}

bool test_native_session_identity_rejects_worker_only_pid() {
  namespace store = ecnuvpn::vpn_engine;

  const int worker_pid = current_process_id();

  store::NativeSessionRecord worker_only;
  worker_only.pid = worker_pid;
  worker_only.supervisor_pid = -1;

  store::NativeSessionRecord durable_supervisor;
  durable_supervisor.pid = worker_pid;
  durable_supervisor.supervisor_pid = worker_pid + 1000;

  bool ok = true;
  ok = expect(!store::native_session_identity_can_outlive_process(
                  worker_only, worker_pid),
              "native helper recorder must reject a worker-only pid identity") &&
       ok;
  ok = expect(store::native_session_identity_can_outlive_process(
                  durable_supervisor, worker_pid),
              "native helper recorder should accept a distinct supervisor pid identity") &&
       ok;

  return ok;
}

bool test_event_recorder_persists_durable_supervisor_identity(
    const std::filesystem::path &root) {
  namespace store = ecnuvpn::vpn_engine;

  const std::string config_dir = (root / "event-recorder-supervisor").string();
  const int durable_supervisor_pid = current_process_id() + 1000;
  store::clear_native_session_state(config_dir);

  store::NativeSessionRecord seed;
  seed.pid = -1;
  seed.supervisor_pid = durable_supervisor_pid;
  seed.server = "vpn.example.edu";
  seed.route_count = 2;

  store::NativeSessionEventRecorder recorder(config_dir, seed);
  recorder.emit(native_event(
      "cstp.connected", "info", "CSTP connect succeeded",
      {{"interface", "tun-native0"}, {"internal_ip", "10.8.0.23"}}));
  recorder.emit(
      native_event("packet.loop.started", "info", "packet loop started"));

  store::NativeSessionProbe probe;
  probe.is_process_alive = [durable_supervisor_pid](int pid) {
    return pid == durable_supervisor_pid;
  };

  store::NativeSessionSnapshot snapshot =
      store::read_native_session_snapshot(config_dir, probe);

  bool ok = true;
  ok = expect(snapshot.running,
              "native recorder should use a durable supervisor pid for liveness") &&
       ok;
  ok = expect(snapshot.pid == -1,
              "native recorder should not require a helper worker pid") &&
       ok;
  ok = expect(snapshot.supervisor_pid == durable_supervisor_pid,
              "native recorder should persist the durable supervisor pid") &&
       ok;
  ok = expect(snapshot.network_ready,
              "durable supervisor native session should report readiness from session state") &&
       ok;

  return ok;
}

bool test_clear_native_session_states_clears_known_config_dirs(
    const std::filesystem::path &root) {
  namespace store = ecnuvpn::vpn_engine;

  const std::string config_dir = (root / "known-config-dir").string();
  store::clear_native_session_state(config_dir);

  bool ok = true;
  ok = expect(store::save_native_session_state(config_dir, ready_record()),
              "saving native state for known config dir should succeed") &&
       ok;
  ok = expect(std::filesystem::exists(
                  store::native_session_state_path(config_dir)),
              "native state should exist before known-dir cleanup") &&
       ok;

  store::clear_native_session_states({"", config_dir, config_dir});
  ok = expect(!std::filesystem::exists(
                  store::native_session_state_path(config_dir)),
              "known-dir cleanup should remove native-session-state.json") &&
       ok;
  ok = expect(!std::filesystem::exists(store::route_ready_path(config_dir)),
              "known-dir cleanup should remove route-ready marker") &&
       ok;

  return ok;
}

bool test_marker_only_malformed_stopped_and_stale_are_disconnected(
    const std::filesystem::path &root) {
  namespace store = ecnuvpn::vpn_engine;

  const std::string config_dir = root.string();
  bool ok = true;

  store::clear_native_session_state(config_dir);
  write_text(store::route_ready_path(config_dir), "tun-native0\n10.8.0.23\n");
  store::NativeSessionSnapshot marker_only =
      store::read_native_session_snapshot(config_dir,
                                          live_current_process_probe());
  ok = expect(!marker_only.running,
              "route-ready marker alone must not make native status connected") &&
       ok;
  ok = expect(!marker_only.network_ready,
              "route-ready marker alone must not make native status ready") &&
       ok;

  write_text(store::native_session_state_path(config_dir), "{not json");
  store::NativeSessionSnapshot malformed =
      store::read_native_session_snapshot(config_dir,
                                          live_current_process_probe());
  ok = expect(!malformed.running,
              "malformed native JSON must not report running") &&
       ok;

  store::NativeSessionRecord stopped = ready_record();
  stopped.session.stopped();
  ok = expect(store::save_native_session_state(config_dir, stopped),
              "saving stopped native state should succeed") &&
       ok;
  store::NativeSessionSnapshot stopped_snapshot =
      store::read_native_session_snapshot(config_dir,
                                          live_current_process_probe());
  ok = expect(!stopped_snapshot.running,
              "stopped native JSON must not report running") &&
       ok;
  ok = expect(!stopped_snapshot.network_ready,
              "stopped native JSON must not report network-ready") &&
       ok;

  store::NativeSessionRecord stale = ready_record();
  stale.pid = current_process_id() + 100000;
  ok = expect(store::save_native_session_state(config_dir, stale),
              "saving stale native state should succeed") &&
       ok;
  store::NativeSessionSnapshot stale_snapshot =
      store::read_native_session_snapshot(config_dir,
                                          live_current_process_probe());
  ok = expect(!stale_snapshot.running,
              "stale native pid must not report running") &&
       ok;

  return ok;
}

} // namespace

bool test_native_error_codes_map_to_contract_codes() {
  namespace store = ecnuvpn::vpn_engine;
  bool ok = true;

  // Direct internal codes map to their canonical contract code.
  ok = expect(store::map_native_error_to_contract_code("tls_verify_failed",
                                                       "certificate rejected") ==
                  "tls_verify_failed",
              "tls_verify_failed should pass through") &&
       ok;
  ok = expect(store::map_native_error_to_contract_code("auth_failed",
                                                       "login failed") ==
                  "auth_failed",
              "auth_failed should pass through") &&
       ok;
  ok = expect(store::map_native_error_to_contract_code("unsupported_auth_flow",
                                                       "") == "auth_failed",
              "unsupported_auth_flow should collapse to auth_failed") &&
       ok;
  ok = expect(store::map_native_error_to_contract_code("unsupported_dtls",
                                                       "") == "unsupported_dtls",
              "unsupported_dtls should pass through") &&
       ok;

  // Platform device failures are recognized via message hints.
  ok = expect(store::map_native_error_to_contract_code(
                  "native_packet_device_factory_failed",
                  "Wintun adapter could not be opened") == "wintun_missing",
              "wintun device failure should map to wintun_missing") &&
       ok;
  ok = expect(store::map_native_error_to_contract_code(
                  "utun_permission_denied", "") == "utun_permission_denied",
              "utun_permission_denied should pass through") &&
       ok;
  ok = expect(store::map_native_error_to_contract_code(
                  "native_packet_device_factory_failed",
                  "utun open failed: permission denied") ==
                  "utun_permission_denied",
              "utun permission message should map to utun_permission_denied") &&
       ok;

  // Unknown codes are returned unchanged so the frontend can fall back.
  ok = expect(store::map_native_error_to_contract_code("protocol_error",
                                                       "bad frame") ==
                  "protocol_error",
              "unrecognized codes should pass through unchanged") &&
       ok;

  return ok;
}

int main() {
  namespace fs = std::filesystem;

  const fs::path root =
      fs::temp_directory_path() /
      ("ecnuvpn-native-helper-session-test-" +
       std::to_string(current_process_id()));

  fs::remove_all(root);
  fs::create_directories(root);

  bool ok = true;
  ok = test_ready_state_uses_native_json_and_emits_compat_marker(root) && ok;
  ok = test_tunnel_configured_without_packet_loop_is_not_network_ready(root) &&
       ok;
  ok = test_missing_liveness_probe_does_not_trust_pid(root) && ok;
  ok = test_event_recorder_persists_worker_session_events(root) && ok;
  ok = test_native_session_identity_rejects_worker_only_pid() && ok;
  ok = test_event_recorder_persists_durable_supervisor_identity(root) && ok;
  ok = test_clear_native_session_states_clears_known_config_dirs(root) && ok;
  ok = test_marker_only_malformed_stopped_and_stale_are_disconnected(root) &&
       ok;
  ok = test_native_error_codes_map_to_contract_codes() && ok;

  fs::remove_all(root);
  return ok ? 0 : 1;
}
