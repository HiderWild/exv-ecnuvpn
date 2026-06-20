// Auth interaction coordinator — bridges aggregate-auth interaction prompts
// (group_select / challenge / two-factor) raised during the prepared-handshake
// branch of run_desktop_connect_job() to the desktop-RPC API surface
// (vpn.authInteraction.get / vpn.authInteraction.respond).
//
// The connect pipeline runs NativeHandshakeJob BEFORE the TunnelController is
// initialized. CoreSessionRunner already maintains a similar handler that the
// runner installs into engine deps; that handler is unreachable during the
// prepared-handshake window because no runner exists yet. Without a
// connect-job-scoped coordinator, the handshake fails immediately on
// auth_group_required / auth_challenge_required and the user has no way to
// continue. See docs/AGGREGATE_AUTH_EMPTY_RESPONSE_FIX_PLAN.md §6.

#pragma once

#include "vpn_engine/protocol/session.hpp"

#include <condition_variable>
#include <memory>
#include <mutex>
#include <optional>
#include <stop_token>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace app_api {

struct PendingAuthInteraction {
  std::string id;
  std::string kind;
  std::string label;
  std::string input_type;
  std::vector<std::string> options;
};

// Thread-safe holder for one in-flight aggregate-auth interaction. The
// handshake thread calls handle() and blocks until the user replies via
// respond(); the RPC handler thread reads pending() to render the prompt.
class AuthInteractionCoordinator {
public:
  AuthInteractionCoordinator() = default;
  ~AuthInteractionCoordinator();

  AuthInteractionCoordinator(const AuthInteractionCoordinator &) = delete;
  AuthInteractionCoordinator &operator=(
      const AuthInteractionCoordinator &) = delete;

  // Publish `request` as the pending prompt and block until respond() supplies
  // a value, cancel() fires, or the supplied stop token is requested. Returns
  // an AuthInteractionResponse with ok=true only when the user actually
  // answered; cancel / stop / mismatched id all yield ok=false so the
  // production_transport layer can map this to auth_group_required /
  // auth_challenge_required gracefully.
  vpn_engine::protocol::AuthInteractionResponse handle(
      const vpn_engine::protocol::AuthInteractionRequest &request,
      std::stop_token stop = {});

  // RPC handler thread: returns a snapshot of the current prompt for
  // serialization to the renderer, or std::nullopt if no prompt is pending.
  std::optional<PendingAuthInteraction> pending() const;

  // RPC handler thread: deliver the user's response. Returns true if the
  // response matched the pending id (handle() will unblock with ok=true);
  // false otherwise (pending may have changed or no prompt was active).
  bool respond(const std::string &id, const std::string &value);

  // Wake any waiter with ok=false. Idempotent. Called automatically from
  // ~AuthInteractionCoordinator() so a handshake job that returns early
  // never strands its own waiters.
  void cancel();

private:
  struct Response {
    std::string id;
    std::string value;
  };

  mutable std::mutex mu_;
  std::condition_variable cv_;
  std::optional<PendingAuthInteraction> pending_;
  std::optional<Response> response_;
  bool cancelled_ = false;
};

// Process-global slot for the coordinator owned by the active connect job.
// At most one connect attempt is in flight at a time (gated upstream by
// VpnConnectJobOwner), so a single shared_ptr is sufficient. The
// vpn.authInteraction.get / vpn.authInteraction.respond RPC handlers consult
// this slot before falling through to the TunnelController, which is how a
// prompt raised during prepared-handshake reaches the renderer.
void set_active_connect_auth_coordinator(
    std::shared_ptr<AuthInteractionCoordinator> coordinator);
std::shared_ptr<AuthInteractionCoordinator>
get_active_connect_auth_coordinator();
bool clear_active_connect_auth_coordinator_if_current(
    const std::shared_ptr<AuthInteractionCoordinator> &coordinator);

} // namespace app_api
} // namespace ecnuvpn
