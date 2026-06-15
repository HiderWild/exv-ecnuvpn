#include "core/tunnel_controller/tunnel_controller_impl.hpp"

namespace exv::core {

// ================================================================
// Reconnect decision
// ================================================================

void TunnelController::Impl::attempt_reconnect(const ErrorInfo& error) {
        auto decision = reconnect_policy_.decide(
            error, intent_, phase_, reconnect_attempts_);

        if (decision.should_retry) {
            ++reconnect_attempts_;
            transition_to(TunnelPhase::Reconnecting);
            scheduler_.schedule(decision.delay, [this] {
                on_reconnect_timer_fired();
            });
        } else {
            set_error(error);
            transition_to(TunnelPhase::Failed);
        }
    }

} // namespace exv::core
