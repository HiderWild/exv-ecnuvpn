#include "reconnect_policy.hpp"
#include <random>
#include <algorithm>
#include <cmath>

namespace exv::core {

ReconnectPolicy::ReconnectPolicy(ReconnectConfig config) : config_(config) {}

ReconnectDecision ReconnectPolicy::decide(
    const ErrorInfo& error,
    const UserIntent& intent,
    TunnelPhase current_phase,
    int attempt_count
) const {
    ReconnectDecision decision;

    // User explicitly doesn't want connection
    if (!intent.desired_connected) {
        decision.should_retry = false;
        decision.reason_code = "user_not_desired";
        return decision;
    }

    // User manually disconnected
    if (intent.user_disconnect_reason.has_value()) {
        decision.should_retry = false;
        decision.reason_code = "user_disconnect";
        return decision;
    }

    // Non-recoverable errors never retry
    if (!error.recoverable) {
        decision.should_retry = false;
        decision.reason_code = "non_recoverable:" + error.code;
        return decision;
    }

    // Auto-reconnect disabled
    if (!intent.auto_reconnect) {
        decision.should_retry = false;
        decision.reason_code = "auto_reconnect_disabled";
        return decision;
    }

    // Max attempts reached
    if (config_.max_attempts > 0 && attempt_count >= config_.max_attempts) {
        decision.should_retry = false;
        decision.reason_code = "max_attempts_reached";
        return decision;
    }

    // Should retry with backoff
    decision.should_retry = true;
    decision.reason_code = "recoverable_error";
    decision.delay = next_delay();
    decision.keep_helper_session = true;
    decision.keep_network_config = true;

    return decision;
}

void ReconnectPolicy::reset() {
    attempt_ = 0;
}

int ReconnectPolicy::attempt_count() const {
    return attempt_;
}

std::chrono::milliseconds ReconnectPolicy::next_delay() const {
    // Exponential backoff: base * 2^attempt, capped at max
    double delay_ms = config_.base_delay.count() * std::pow(2.0, attempt_);
    delay_ms = std::min(delay_ms, static_cast<double>(config_.max_delay.count()));

    // Add jitter: plus/minus jitter_factor
    double jitter;
    if (config_.random) {
        jitter = (config_.random() * 2.0 - 1.0) * config_.jitter_factor;
    } else {
        std::random_device rd;
        std::mt19937 gen(rd());
        std::uniform_real_distribution<> dis(-config_.jitter_factor, config_.jitter_factor);
        jitter = dis(gen);
    }
    delay_ms *= (1.0 + jitter);

    return std::chrono::milliseconds(static_cast<int64_t>(delay_ms));
}

std::chrono::steady_clock::time_point ReconnectPolicy::now() const {
    if (config_.clock) return config_.clock();
    return std::chrono::steady_clock::now();
}

} // namespace exv::core
