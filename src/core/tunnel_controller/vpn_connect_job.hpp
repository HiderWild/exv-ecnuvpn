#pragma once

#include "core/tunnel_controller/connect_intent.hpp"

#include <cstdint>
#include <functional>
#include <mutex>
#include <optional>
#include <stop_token>
#include <string>
#include <thread>

namespace exv::core {

struct VpnConnectJobState {
  std::string job_id;
  std::string phase;
  bool accepted = false;
  bool active = false;
  bool cancelling = false;
  bool user_cancelled = false;
  bool coalesced = false;
  bool desired_connected = false;
  std::uint64_t intent_epoch = 0;
  std::string last_error_code;
  std::string last_error_message;
};

class VpnConnectJobOwner {
public:
  using RunFn = std::function<void(std::stop_token, std::uint64_t)>;

  VpnConnectJobOwner() = default;
  ~VpnConnectJobOwner();

  VpnConnectJobOwner(const VpnConnectJobOwner&) = delete;
  VpnConnectJobOwner& operator=(const VpnConnectJobOwner&) = delete;

  VpnConnectJobState submit_connect(PendingConnectRequest request, RunFn run);
  VpnConnectJobState submit_disconnect(std::string reason);
  VpnConnectJobState snapshot() const;
  bool request_cancel(std::string reason);
  void reconcile_after_idle();

private:
  VpnConnectJobState start_job_locked(RunFn run);
  void join_finished_locked();
  void mark_finished(std::uint64_t epoch,
                     std::string error_code,
                     std::string error_message);

  mutable std::mutex mutex_;
  VpnWorkflowIntent intent_;
  VpnConnectJobState state_;
  std::optional<RunFn> pending_run_;
  std::jthread active_thread_;
  std::uint64_t next_job_number_ = 0;
  std::uint64_t represented_epoch_ = 0;
  std::uint64_t active_job_epoch_ = 0;
};

} // namespace exv::core
