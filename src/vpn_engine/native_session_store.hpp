#pragma once

#include "vpn_engine/event_sink.hpp"
#include "vpn_engine/session_state.hpp"

#include <functional>
#include <mutex>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace vpn_engine {

struct NativeSessionRecord {
  std::string engine = "native";
  SessionState session;
  int pid = -1;
  int supervisor_pid = -1;
  std::string server;
  int route_count = 0;
  int retry_limit = 0;
};

struct NativeSessionProbe {
  std::function<bool(int)> is_process_alive;
};

struct NativeSessionSnapshot {
  bool running = false;
  int pid = -1;
  int supervisor_pid = -1;
  bool network_ready = false;
  std::string interface_name;
  std::string internal_ip;
  std::string server;
  int route_count = 0;
  int retry_limit = 0;
  std::string failure_code;
  std::string failure_message;
};

class NativeSessionEventRecorder final : public EventSink {
public:
  NativeSessionEventRecorder(std::string config_dir,
                             NativeSessionRecord record);

  void emit(const VpnEngineEvent &event) override;
  bool persist_current();
  bool mark_stopped();

private:
  std::string config_dir_;
  NativeSessionRecord record_;
  std::mutex mu_;
};

std::string native_session_state_path(const std::string &config_dir);
std::string route_ready_path(const std::string &config_dir);

bool save_native_session_state(const std::string &config_dir,
                               const NativeSessionRecord &record);
bool load_native_session_state(const std::string &config_dir,
                               NativeSessionRecord *record);
bool native_session_identity_can_outlive_process(
    const NativeSessionRecord &record, int process_pid);
void clear_native_session_state(const std::string &config_dir);
void clear_native_session_states(const std::vector<std::string> &config_dirs);

NativeSessionSnapshot
read_native_session_snapshot(const std::string &config_dir,
                             const NativeSessionProbe &probe);

} // namespace vpn_engine
} // namespace ecnuvpn
