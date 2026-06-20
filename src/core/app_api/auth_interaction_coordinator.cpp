#include "core/app_api/auth_interaction_coordinator.hpp"

#include <utility>

namespace exv {
namespace app_api {
namespace {

std::mutex &active_coordinator_mutex() {
  static std::mutex m;
  return m;
}

std::shared_ptr<AuthInteractionCoordinator> &active_coordinator_slot() {
  static std::shared_ptr<AuthInteractionCoordinator> slot;
  return slot;
}

} // namespace

AuthInteractionCoordinator::~AuthInteractionCoordinator() {
  cancel();
}

vpn_engine::protocol::AuthInteractionResponse
AuthInteractionCoordinator::handle(
    const vpn_engine::protocol::AuthInteractionRequest &request,
    std::stop_token stop) {
  {
    std::lock_guard<std::mutex> lock(mu_);
    pending_ = PendingAuthInteraction{request.id, request.kind, request.label,
                                      request.input_type, request.options};
    response_.reset();
    cancelled_ = false;
  }
  cv_.notify_all();

  std::stop_callback stop_cb(stop, [this] {
    {
      std::lock_guard<std::mutex> lock(mu_);
      cancelled_ = true;
    }
    cv_.notify_all();
  });

  vpn_engine::protocol::AuthInteractionResponse out;
  {
    std::unique_lock<std::mutex> lock(mu_);
    cv_.wait(lock, [this] {
      return cancelled_ || (response_ && response_->id == pending_->id);
    });

    if (cancelled_ || !response_) {
      out.ok = false;
    } else {
      out.ok = true;
      out.value = response_->value;
    }
    pending_.reset();
    response_.reset();
  }
  return out;
}

std::optional<PendingAuthInteraction>
AuthInteractionCoordinator::pending() const {
  std::lock_guard<std::mutex> lock(mu_);
  return pending_;
}

bool AuthInteractionCoordinator::respond(const std::string &id,
                                         const std::string &value) {
  {
    std::lock_guard<std::mutex> lock(mu_);
    if (!pending_ || pending_->id != id) {
      return false;
    }
    response_ = Response{id, value};
  }
  cv_.notify_all();
  return true;
}

void AuthInteractionCoordinator::cancel() {
  {
    std::lock_guard<std::mutex> lock(mu_);
    cancelled_ = true;
  }
  cv_.notify_all();
}

void set_active_connect_auth_coordinator(
    std::shared_ptr<AuthInteractionCoordinator> coordinator) {
  std::lock_guard<std::mutex> lock(active_coordinator_mutex());
  active_coordinator_slot() = std::move(coordinator);
}

std::shared_ptr<AuthInteractionCoordinator>
get_active_connect_auth_coordinator() {
  std::lock_guard<std::mutex> lock(active_coordinator_mutex());
  return active_coordinator_slot();
}

bool clear_active_connect_auth_coordinator_if_current(
    const std::shared_ptr<AuthInteractionCoordinator> &coordinator) {
  if (!coordinator) {
    return false;
  }

  std::lock_guard<std::mutex> lock(active_coordinator_mutex());
  if (active_coordinator_slot().get() != coordinator.get()) {
    return false;
  }
  active_coordinator_slot().reset();
  return true;
}

} // namespace app_api
} // namespace exv
