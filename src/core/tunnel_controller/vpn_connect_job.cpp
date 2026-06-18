#include "core/tunnel_controller/vpn_connect_job.hpp"

#include <exception>
#include <utility>

namespace exv::core {

VpnConnectJobOwner::~VpnConnectJobOwner() {
  if (active_thread_.joinable()) {
    active_thread_.request_stop();
    active_thread_.join();
  }
}

VpnConnectJobState VpnConnectJobOwner::submit_connect(PendingConnectRequest request,
                                                      RunFn run) {
  std::lock_guard<std::mutex> lock(mutex_);
  join_finished_locked();
  intent_.desired = DesiredVpnIntent::Connect;
  intent_.epoch += 1;
  intent_.pending_connect = std::move(request);
  pending_run_ = std::move(run);

  if (state_.active) {
    state_.accepted = true;
    state_.coalesced = true;
    state_.desired_connected = true;
    state_.intent_epoch = intent_.epoch;
    return state_;
  }

  return start_job_locked(*pending_run_);
}

VpnConnectJobState VpnConnectJobOwner::submit_disconnect(std::string reason) {
  std::lock_guard<std::mutex> lock(mutex_);
  join_finished_locked();
  intent_.desired = DesiredVpnIntent::Disconnect;
  intent_.epoch += 1;
  state_.accepted = true;
  state_.desired_connected = false;
  state_.intent_epoch = intent_.epoch;
  pending_run_.reset();

  if (state_.active) {
    state_.cancelling = true;
    state_.user_cancelled = reason == "user_cancelled_connect";
    active_thread_.request_stop();
  }
  return state_;
}

VpnConnectJobState VpnConnectJobOwner::snapshot() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return state_;
}

bool VpnConnectJobOwner::request_cancel(std::string reason) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (!state_.active) {
    return false;
  }
  state_.cancelling = true;
  state_.user_cancelled = reason == "user_cancelled_connect";
  active_thread_.request_stop();
  return true;
}

void VpnConnectJobOwner::reconcile_after_idle() {
  RunFn run;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    join_finished_locked();
    if (state_.active ||
        intent_.desired != DesiredVpnIntent::Connect ||
        !pending_run_.has_value() ||
        intent_.epoch <= represented_epoch_) {
      return;
    }
    run = *pending_run_;
    start_job_locked(run);
  }
}

VpnConnectJobState VpnConnectJobOwner::start_job_locked(RunFn run) {
  join_finished_locked();
  represented_epoch_ = intent_.epoch;
  active_job_epoch_ = intent_.epoch;
  state_ = {};
  state_.accepted = true;
  state_.active = true;
  state_.phase = "connecting";
  state_.desired_connected = true;
  state_.intent_epoch = intent_.epoch;
  state_.job_id = "connect-" + std::to_string(++next_job_number_);
  const std::uint64_t epoch = intent_.epoch;

  active_thread_ = std::jthread(
      [this, run = std::move(run), epoch](std::stop_token stop) mutable {
        std::string error_code;
        std::string error_message;
        try {
          run(stop, epoch);
        } catch (const std::exception& error) {
          error_code = "job_failed";
          error_message = error.what();
        } catch (...) {
          error_code = "job_failed";
          error_message = "Unhandled VPN connect job failure";
        }
        mark_finished(epoch, std::move(error_code), std::move(error_message));
      });
  return state_;
}

void VpnConnectJobOwner::join_finished_locked() {
  if (active_thread_.joinable() && !state_.active) {
    active_thread_.join();
  }
}

void VpnConnectJobOwner::mark_finished(std::uint64_t epoch,
                                       std::string error_code,
                                       std::string error_message) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (active_job_epoch_ != epoch) {
    return;
  }
  state_.active = false;
  state_.phase = "idle";
  state_.cancelling = false;
  if (!state_.user_cancelled) {
    state_.last_error_code = std::move(error_code);
    state_.last_error_message = std::move(error_message);
  }
}

} // namespace exv::core
