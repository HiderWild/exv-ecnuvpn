#pragma once

#include "core/config/config.hpp"
#include "vpn_engine/event_sink.hpp"
#include "vpn_engine/engine.hpp"
#include "vpn_engine/packet_device.hpp"
#include "vpn_engine/protocol/session.hpp"

#include <atomic>
#include <condition_variable>
#include <functional>
#include <memory>
#include <mutex>
#include <nlohmann/json.hpp>
#include <thread>

namespace ecnuvpn {
namespace vpn_engine {

ValidationResult validate_native_config(const Config &cfg);
VpnEngineConfig make_native_config(const Config &cfg,
                                   const std::string &plaintext_password);
nlohmann::json event_to_json(const VpnEngineEvent &event);
nlohmann::json status_to_json(const VpnEngineStatus &status);

struct NativeVpnEngineDependencies {
  std::function<std::unique_ptr<protocol::ProtocolTransport>()>
      transport_factory;
  std::function<std::unique_ptr<PacketDevice>()> packet_device_factory;
  std::function<ValidationResult(const TunnelMetadata &, DeviceConfig *)>
      network_configurator;
  // Optional observer owned by the caller. It must outlive the session and its
  // emit() implementation must be safe from both the caller thread and the
  // packet-loop thread.
  EventSink *event_sink = nullptr;
};

NativeVpnEngineDependencies default_native_engine_dependencies();

class NativeVpnEngineSession final : public VpnEngineSession,
                                     private protocol::CancellationToken {
public:
  explicit NativeVpnEngineSession(VpnEngineConfig config);
  NativeVpnEngineSession(VpnEngineConfig config,
                         NativeVpnEngineDependencies dependencies);
  ~NativeVpnEngineSession() override;

  NativeVpnEngineSession(const NativeVpnEngineSession &) = delete;
  NativeVpnEngineSession &operator=(const NativeVpnEngineSession &) = delete;

  ValidationResult start() override;
  void stop() override;
  VpnEngineStatus status() const override;

private:
  class LoopEventSink;

  bool is_cancelled() const override;
  void emit_event(std::string type, std::string level, std::string message,
                  std::map<std::string, std::string> fields = {});
  void on_loop_event(const VpnEngineEvent &event);
  void run_packet_loop();
  void set_failure_locked(const ValidationResult &failure);

  VpnEngineConfig config_;
  NativeVpnEngineDependencies dependencies_;
  std::unique_ptr<protocol::ProtocolTransport> transport_;
  std::unique_ptr<protocol::ProtocolSession> protocol_session_;
  std::unique_ptr<PacketDevice> packet_device_;
  std::unique_ptr<LoopEventSink> loop_event_sink_;
  DeviceConfig packet_device_config_;
  VpnEngineStatus status_;

  mutable std::mutex mu_;
  std::condition_variable startup_cv_;
  std::thread packet_loop_thread_;
  std::atomic<bool> cancel_requested_{false};
  bool loop_started_ = false;
  bool loop_finished_ = false;
  ValidationResult loop_start_result_;
};

} // namespace vpn_engine
} // namespace ecnuvpn
