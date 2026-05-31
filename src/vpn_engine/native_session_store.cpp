#include "vpn_engine/native_session_store.hpp"

#include <atomic>
#include <chrono>
#include <filesystem>
#include <functional>
#include <fstream>
#include <iterator>
#include <thread>
#include <utility>

namespace ecnuvpn {
namespace vpn_engine {
namespace {

constexpr const char *kNativeSessionStateFile = "native-session-state.json";
constexpr const char *kRouteReadyFile = "route-ready";

std::string path_for(const std::string &config_dir, const char *filename) {
  return (std::filesystem::path(config_dir) / filename).string();
}

bool ensure_parent_dir(const std::string &path) {
  std::filesystem::path fs_path(path);
  std::error_code ec;
  if (!fs_path.has_parent_path())
    return true;
  std::filesystem::create_directories(fs_path.parent_path(), ec);
  return !ec;
}

std::filesystem::path unique_temp_path(const std::filesystem::path &target) {
  static std::atomic<unsigned long long> counter{0};
  const auto ticks = std::chrono::steady_clock::now()
                         .time_since_epoch()
                         .count();
  std::filesystem::path tmp = target;
  tmp += ".tmp." + std::to_string(ticks) + "." +
         std::to_string(counter.fetch_add(1, std::memory_order_relaxed)) +
         "." + std::to_string(
                   std::hash<std::thread::id>{}(std::this_thread::get_id()));
  return tmp;
}

bool write_file_atomicish(const std::string &path, const std::string &content) {
  if (!ensure_parent_dir(path))
    return false;

  std::filesystem::path target(path);
  std::filesystem::path tmp = unique_temp_path(target);

  {
    std::ofstream out(tmp, std::ios::binary | std::ios::trunc);
    if (!out.is_open())
      return false;
    out << content;
    out.close();
    if (!out.good()) {
      std::error_code remove_ec;
      std::filesystem::remove(tmp, remove_ec);
      return false;
    }
  }

  std::error_code ec;
  std::filesystem::rename(tmp, target, ec);
  if (ec) {
    ec.clear();
    std::filesystem::copy_file(
        tmp, target, std::filesystem::copy_options::overwrite_existing, ec);
    if (!ec) {
      std::error_code remove_ec;
      std::filesystem::remove(tmp, remove_ec);
      return true;
    }

    std::error_code remove_ec;
    std::filesystem::remove(tmp, remove_ec);
    return false;
  }
  return true;
}

std::string read_file(const std::string &path) {
  std::ifstream in(path, std::ios::binary);
  if (!in.is_open())
    return "";
  return std::string(std::istreambuf_iterator<char>(in),
                     std::istreambuf_iterator<char>());
}

void remove_file(const std::string &path) {
  std::error_code ec;
  std::filesystem::remove(path, ec);
}

bool phase_from_json(const nlohmann::json &value, SessionPhase *phase) {
  if (!phase || !value.is_string())
    return false;

  const std::string text = value.get<std::string>();
  if (text == "idle")
    *phase = SessionPhase::idle;
  else if (text == "authenticating")
    *phase = SessionPhase::authenticating;
  else if (text == "authenticated")
    *phase = SessionPhase::authenticated;
  else if (text == "configuring_network")
    *phase = SessionPhase::configuring_network;
  else if (text == "packet_loop")
    *phase = SessionPhase::packet_loop;
  else if (text == "reconnecting")
    *phase = SessionPhase::reconnecting;
  else if (text == "stopping")
    *phase = SessionPhase::stopping;
  else if (text == "stopped")
    *phase = SessionPhase::stopped;
  else if (text == "failed")
    *phase = SessionPhase::failed;
  else
    return false;
  return true;
}

std::vector<std::string> string_array_from_json(const nlohmann::json &value) {
  std::vector<std::string> result;
  if (!value.is_array())
    return result;
  for (const auto &entry : value) {
    if (entry.is_string())
      result.push_back(entry.get<std::string>());
  }
  return result;
}

bool tunnel_metadata_from_json(const nlohmann::json &j,
                               TunnelMetadata *metadata) {
  if (!metadata || !j.is_object())
    return false;

  metadata->interface_name = j.value("interface_name", std::string());
  metadata->interface_index = j.value("interface_index", -1);
  metadata->internal_ip4_address =
      j.value("internal_ip4_address", std::string());
  metadata->internal_ip4_netmask =
      j.value("internal_ip4_netmask", std::string());
  metadata->mtu = j.value("mtu", 1290);
  metadata->routes =
      string_array_from_json(j.value("routes", nlohmann::json::array()));
  metadata->server_bypass_ips = string_array_from_json(
      j.value("server_bypass_ips", nlohmann::json::array()));
  return true;
}

bool session_state_from_json(const nlohmann::json &j, SessionState *session) {
  if (!session || !j.is_object())
    return false;

  SessionPhase phase = SessionPhase::idle;
  if (!phase_from_json(j.value("phase", nlohmann::json()), &phase))
    return false;

  SessionState parsed;
  parsed.phase = phase;
  parsed.tunnel_ready = j.value("tunnel_ready", false);
  parsed.packet_loop_ready = j.value("packet_loop_ready", false);
  parsed.last_event_message = j.value("last_event_message", std::string());

  if (j.contains("tunnel_metadata")) {
    if (!tunnel_metadata_from_json(j.at("tunnel_metadata"), &parsed.tunnel))
      return false;
  }

  if (j.contains("failure") && j["failure"].is_object()) {
    parsed.failure_code = j["failure"].value("code", std::string());
    parsed.failure_message = j["failure"].value("message", std::string());
  }

  *session = std::move(parsed);
  return true;
}

nlohmann::json record_to_json(const NativeSessionRecord &record) {
  return nlohmann::json{{"engine",
                         record.engine.empty() ? "native" : record.engine},
                        {"pid", record.pid},
                        {"supervisor_pid", record.supervisor_pid},
                        {"server", record.server},
                        {"route_count", record.route_count},
                        {"retry_limit", record.retry_limit},
                        {"session", session_state_to_json(record.session)}};
}

bool record_from_json(const nlohmann::json &j, NativeSessionRecord *record) {
  if (!record || !j.is_object())
    return false;

  NativeSessionRecord parsed;
  parsed.engine = j.value("engine", std::string("native"));
  if (parsed.engine != "native")
    return false;

  parsed.pid = j.value("pid", -1);
  parsed.supervisor_pid = j.value("supervisor_pid", -1);
  parsed.server = j.value("server", std::string());
  parsed.route_count = j.value("route_count", 0);
  parsed.retry_limit = j.value("retry_limit", 0);

  const nlohmann::json &session_json =
      j.contains("session") ? j.at("session") : j;
  if (!session_state_from_json(session_json, &parsed.session))
    return false;

  *record = std::move(parsed);
  return true;
}

bool native_session_ready(const SessionState &session) {
  return session.network_ready() && !session.tunnel.interface_name.empty() &&
         !session.tunnel.internal_ip4_address.empty();
}

bool native_session_active(const SessionState &session) {
  switch (session.phase) {
  case SessionPhase::authenticating:
  case SessionPhase::authenticated:
  case SessionPhase::configuring_network:
  case SessionPhase::packet_loop:
  case SessionPhase::reconnecting:
  case SessionPhase::stopping:
    return true;
  case SessionPhase::idle:
  case SessionPhase::stopped:
  case SessionPhase::failed:
    return false;
  }
  return false;
}

bool process_alive(const NativeSessionProbe &probe, int pid) {
  if (pid <= 0)
    return false;
  if (!probe.is_process_alive)
    return false;
  return probe.is_process_alive(pid);
}

std::string event_field(const VpnEngineEvent &event, const char *key) {
  auto it = event.fields.find(key);
  if (it == event.fields.end())
    return "";
  return it->second;
}

bool is_failure_event(const std::string &type) {
  constexpr const char *kFailedSuffix = ".failed";
  const std::string suffix(kFailedSuffix);
  return type.size() >= suffix.size() &&
         type.compare(type.size() - suffix.size(), suffix.size(), suffix) == 0;
}

void write_route_ready_marker(const std::string &config_dir,
                              const SessionState &session) {
  if (!native_session_ready(session)) {
    remove_file(route_ready_path(config_dir));
    return;
  }

  write_file_atomicish(route_ready_path(config_dir),
                       session.tunnel.interface_name + "\n" +
                           session.tunnel.internal_ip4_address + "\n");
}

} // namespace

NativeSessionEventRecorder::NativeSessionEventRecorder(
    std::string config_dir, NativeSessionRecord record)
    : config_dir_(std::move(config_dir)), record_(std::move(record)) {
  if (record_.engine.empty())
    record_.engine = "native";
}

void NativeSessionEventRecorder::emit(const VpnEngineEvent &event) {
  const std::lock_guard<std::mutex> lock(mu_);

  record_.session.last_event_message = event.message;
  if (event.type == "auth.started") {
    record_.session.auth_started();
  } else if (event.type == "auth.succeeded") {
    record_.session.auth_succeeded();
  } else if (event.type == "cstp.connected") {
    TunnelMetadata metadata = record_.session.tunnel;
    metadata.interface_name = event_field(event, "interface");
    metadata.internal_ip4_address = event_field(event, "internal_ip");
    record_.session.tunnel_configured(metadata);
  } else if (event.type == "packet.loop.started") {
    record_.session.packet_loop_started();
  } else if (is_failure_event(event.type)) {
    std::string code = event_field(event, "code");
    if (code.empty())
      code = event.type;
    record_.session.failed(code, event.message);
  } else {
    return;
  }

  save_native_session_state(config_dir_, record_);
}

bool NativeSessionEventRecorder::persist_current() {
  const std::lock_guard<std::mutex> lock(mu_);
  return save_native_session_state(config_dir_, record_);
}

bool NativeSessionEventRecorder::mark_stopped() {
  const std::lock_guard<std::mutex> lock(mu_);
  record_.session.stopped();
  return save_native_session_state(config_dir_, record_);
}

std::string native_session_state_path(const std::string &config_dir) {
  return path_for(config_dir, kNativeSessionStateFile);
}

std::string route_ready_path(const std::string &config_dir) {
  return path_for(config_dir, kRouteReadyFile);
}

bool save_native_session_state(const std::string &config_dir,
                               const NativeSessionRecord &record) {
  if (!write_file_atomicish(native_session_state_path(config_dir),
                            record_to_json(record).dump(2))) {
    return false;
  }
  write_route_ready_marker(config_dir, record.session);
  return true;
}

bool load_native_session_state(const std::string &config_dir,
                               NativeSessionRecord *record) {
  if (!record)
    return false;

  std::error_code ec;
  if (!std::filesystem::exists(native_session_state_path(config_dir), ec) || ec)
    return false;

  try {
    nlohmann::json j =
        nlohmann::json::parse(read_file(native_session_state_path(config_dir)));
    return record_from_json(j, record);
  } catch (...) {
    return false;
  }
}

bool native_session_identity_can_outlive_process(
    const NativeSessionRecord &record, int process_pid) {
  auto can_outlive = [process_pid](int pid) {
    return pid > 0 && (process_pid <= 0 || pid != process_pid);
  };
  return can_outlive(record.pid) || can_outlive(record.supervisor_pid);
}

void clear_native_session_state(const std::string &config_dir) {
  remove_file(native_session_state_path(config_dir));
  remove_file(route_ready_path(config_dir));
}

void clear_native_session_states(const std::vector<std::string> &config_dirs) {
  for (const std::string &config_dir : config_dirs) {
    if (!config_dir.empty())
      clear_native_session_state(config_dir);
  }
}

NativeSessionSnapshot
read_native_session_snapshot(const std::string &config_dir,
                             const NativeSessionProbe &probe) {
  NativeSessionSnapshot snapshot;

  NativeSessionRecord record;
  if (!load_native_session_state(config_dir, &record))
    return snapshot;

  const bool session_ready = native_session_ready(record.session);
  const bool session_active = native_session_active(record.session);
  snapshot.pid = process_alive(probe, record.pid) ? record.pid : -1;
  snapshot.supervisor_pid =
      process_alive(probe, record.supervisor_pid) ? record.supervisor_pid : -1;
  snapshot.running =
      session_active && (snapshot.pid > 0 || snapshot.supervisor_pid > 0);
  snapshot.network_ready = snapshot.running && session_ready;
  snapshot.interface_name = record.session.tunnel.interface_name;
  snapshot.internal_ip = record.session.tunnel.internal_ip4_address;
  snapshot.server = record.server;
  snapshot.route_count =
      record.route_count > 0
          ? record.route_count
          : static_cast<int>(record.session.tunnel.routes.size());
  snapshot.retry_limit = record.retry_limit;

  if (!snapshot.running) {
    snapshot.pid = -1;
    snapshot.supervisor_pid = -1;
    snapshot.network_ready = false;
  }

  return snapshot;
}

} // namespace vpn_engine
} // namespace ecnuvpn
